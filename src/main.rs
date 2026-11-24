mod ocr;
mod writer;

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use exif::{Exif, In, Rational, Reader, Tag, Value};
use ocr::{normalize_vi, OcrCache};
use writer::{strip_metadata_copy, write_metadata_copy, MetadataPatch};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use walkdir::WalkDir;

const INDEX_HTML: &str = include_str!("../web/index.html");
const OCR_CACHE_FILE: &str = ".exif_map_ocr_cache.json";
static COPY_SEQ: AtomicU64 = AtomicU64::new(1);

const SUPPORTED_EXTS: &[&str] = &[
    "jpg", "jpeg", "tif", "tiff", "heic", "heif", "png", "webp", "avif",
];

#[derive(Clone, Default)]
struct AppState {
    photos: Arc<RwLock<Vec<IndexedPhoto>>>,
    ocr_progress: Arc<RwLock<OcrProgress>>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct OcrProgress {
    running: bool,
    done: usize,
    total: usize,
    from_cache: usize,
    errors: usize,
}

#[derive(Clone)]
struct IndexedPhoto {
    id: String,
    disk_path: PathBuf,
    record: PhotoRecord,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhotoRecord {
    id: String,
    filename: String,
    path: String,
    size_bytes: u64,
    thumb_url: String,
    lat: Option<f64>,
    lon: Option<f64>,
    altitude: Option<f64>,
    datetime: Option<String>,
    camera_make: Option<String>,
    camera_model: Option<String>,
    focal_length: Option<String>,
    f_number: Option<String>,
    iso: Option<u32>,
    ocr_text: Option<String>,
    ocr_status: String,
}

#[derive(Deserialize)]
struct ScanRequest {
    folder: String,
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
}

#[derive(Serialize)]
struct OcrStatusResponse {
    progress: OcrProgress,
    photos: Vec<PhotoRecord>,
}

#[derive(Serialize)]
struct FeatureCollection {
    #[serde(rename = "type")]
    kind: &'static str,
    features: Vec<Feature>,
}

#[derive(Serialize)]
struct Feature {
    #[serde(rename = "type")]
    kind: &'static str,
    geometry: Geometry,
    properties: Properties,
}

#[derive(Serialize)]
struct Geometry {
    #[serde(rename = "type")]
    kind: &'static str,
    /// Theo chuẩn GeoJSON (RFC 7946): [longitude, latitude] hoặc [longitude, latitude, altitude]
    coordinates: Vec<f64>,
}

#[derive(Serialize)]
struct Properties {
    filename: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    datetime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    camera_make: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    camera_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    altitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ocr_text: Option<String>,
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() >= 2 && args[1] != "--serve" && args[1] != "-s" {
        if args[1] == "-h" || args[1] == "--help" {
            eprintln!("Exif_Map — Quản lý ảnh EXIF, OCR gốc hệ điều hành & xuất GeoJSON (chỉ đọc)");
            eprintln!("  Chế độ Web UI: {} [--serve [port]]", args[0]);
            eprintln!(
                "  Chế độ CLI:    {} <file_hoac_thu_muc> [output.geojson] [--ocr \"tu khoa\"]",
                args[0]
            );
            process::exit(0);
        }
        run_cli(&args[1..]);
        return;
    }

    let port: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3030);

    let state = AppState::default();
    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/scan", post(api_scan_folder))
        .route("/api/photos", get(api_list_photos))
        .route("/api/photo/{id}", get(api_serve_photo))
        .route("/api/photo/{id}/metadata", post(api_write_metadata_copy))
        .route("/api/photo/{id}/strip", post(api_strip_metadata_copy))
        .route("/api/photos/strip-batch", post(api_strip_metadata_batch))
        .route("/api/photo/{id}/download", get(api_download_photo))
        .route("/api/ocr/start", post(api_start_ocr))
        .route("/api/ocr/status", get(api_ocr_status))
        .route("/api/search", get(api_search_photos))
        .route("/api/geojson", get(api_geojson))
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Không thể mở cổng local server");

    eprintln!("Exif_Map Web UI (EXIF + OCR chỉ đọc) đang chạy tại: http://{addr}");
    eprintln!("Cache OCR lưu tại: {OCR_CACHE_FILE}");
    eprintln!("Nhấn Ctrl+C để dừng server.");

    axum::serve(listener, app)
        .await
        .expect("Lỗi chạy axum server");
}

async fn serve_index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn api_scan_folder(
    State(state): State<AppState>,
    Json(payload): Json<ScanRequest>,
) -> impl IntoResponse {
    let input = PathBuf::from(payload.folder.trim());
    if !input.exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Không tìm thấy đường dẫn thư mục." })),
        )
            .into_response();
    }

    let cache = OcrCache::load(Path::new(OCR_CACHE_FILE));
    let paths = collect_image_paths(&input);
    let mut indexed = Vec::with_capacity(paths.len());

    for (idx, path) in paths.into_iter().enumerate() {
        let id = format!("local-{idx}");
        let mut record = read_photo_record(&id, &path);
        if let Some(cached_text) = cache.get_valid(&path) {
            record.ocr_text = if cached_text.is_empty() {
                None
            } else {
                Some(cached_text)
            };
            record.ocr_status = "cached".to_string();
        }
        indexed.push(IndexedPhoto {
            id,
            disk_path: path,
            record,
        });
    }

    let records: Vec<PhotoRecord> = indexed.iter().map(|p| p.record.clone()).collect();
    if let Ok(mut guard) = state.photos.write() {
        *guard = indexed;
    }
    if let Ok(mut prog) = state.ocr_progress.write() {
        let cached_count = records.iter().filter(|r| r.ocr_status == "cached").count();
        *prog = OcrProgress {
            running: false,
            done: cached_count,
            total: records.len(),
            from_cache: cached_count,
            errors: 0,
        };
    }

    Json(records).into_response()
}

async fn api_start_ocr(State(state): State<AppState>) -> impl IntoResponse {
    {
        if let Ok(prog) = state.ocr_progress.read() {
            if prog.running {
                return (StatusCode::ACCEPTED, Json(prog.clone())).into_response();
            }
        }
    }

    let jobs: Vec<(String, PathBuf)> = state
        .photos
        .read()
        .map(|g| {
            g.iter()
                .map(|p| (p.id.clone(), p.disk_path.clone()))
                .collect()
        })
        .unwrap_or_default();

    let total = jobs.len();
    if let Ok(mut prog) = state.ocr_progress.write() {
        *prog = OcrProgress {
            running: total > 0,
            done: 0,
            total,
            from_cache: 0,
            errors: 0,
        };
    }

    if total == 0 {
        return (StatusCode::OK, Json(OcrProgress::default())).into_response();
    }

    let bg_state = state.clone();
    tokio::task::spawn_blocking(move || {
        let cache_path = Path::new(OCR_CACHE_FILE);
        let mut cache = OcrCache::load(cache_path);
        let mut cache_dirty = false;

        for (id, path) in jobs {
            let (text_opt, status, hit_cache, had_error) = if let Some(cached) = cache.get_valid(&path) {
                (
                    if cached.is_empty() { None } else { Some(cached) },
                    "cached".to_string(),
                    true,
                    false,
                )
            } else {
                match ocr::recognize_text(&path) {
                    Ok(text) => {
                        cache.insert(&path, text.clone());
                        cache_dirty = true;
                        (
                            if text.is_empty() { None } else { Some(text) },
                            "done".to_string(),
                            false,
                            false,
                        )
                    }
                    Err(_) => (None, "error".to_string(), false, true),
                }
            };

            if let Ok(mut photos) = bg_state.photos.write() {
                if let Some(item) = photos.iter_mut().find(|p| p.id == id) {
                    item.record.ocr_text = text_opt;
                    item.record.ocr_status = status;
                }
            }

            if let Ok(mut prog) = bg_state.ocr_progress.write() {
                prog.done += 1;
                if hit_cache {
                    prog.from_cache += 1;
                }
                if had_error {
                    prog.errors += 1;
                }
            }
        }

        if cache_dirty {
            let _ = cache.save(cache_path);
        }

        if let Ok(mut prog) = bg_state.ocr_progress.write() {
            prog.running = false;
        }
    });

    let current = state
        .ocr_progress
        .read()
        .map(|g| g.clone())
        .unwrap_or_default();
    (StatusCode::ACCEPTED, Json(current)).into_response()
}

async fn api_ocr_status(State(state): State<AppState>) -> impl IntoResponse {
    let progress = state
        .ocr_progress
        .read()
        .map(|g| g.clone())
        .unwrap_or_default();
    let photos = state
        .photos
        .read()
        .map(|g| g.iter().map(|p| p.record.clone()).collect())
        .unwrap_or_default();
    Json(OcrStatusResponse { progress, photos })
}

async fn api_search_photos(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    let needle = normalize_vi(params.q.as_deref().unwrap_or("").trim());
    let records: Vec<PhotoRecord> = state
        .photos
        .read()
        .map(|g| {
            g.iter()
                .map(|p| p.record.clone())
                .filter(|r| {
                    if needle.is_empty() {
                        return true;
                    }
                    let hay = format!(
                        "{} {} {} {} {}",
                        r.filename,
                        r.path,
                        r.camera_make.as_deref().unwrap_or(""),
                        r.camera_model.as_deref().unwrap_or(""),
                        r.ocr_text.as_deref().unwrap_or("")
                    );
                    normalize_vi(&hay).contains(&needle)
                })
                .collect()
        })
        .unwrap_or_default();
    Json(records)
}

async fn api_list_photos(State(state): State<AppState>) -> impl IntoResponse {
    let records: Vec<PhotoRecord> = state
        .photos
        .read()
        .map(|g| g.iter().map(|p| p.record.clone()).collect())
        .unwrap_or_default();
    Json(records)
}

async fn api_serve_photo(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let disk_path = state
        .photos
        .read()
        .ok()
        .and_then(|g| g.iter().find(|p| p.id == id).map(|p| p.disk_path.clone()));

    let Some(path) = disk_path else {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    };

    let Ok(bytes) = fs::read(&path) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Không thể đọc tệp ảnh").into_response();
    };

    let mime = guess_mime(&path);
    ([(header::CONTENT_TYPE, mime)], bytes).into_response()
}


#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WriteMetadataResponse {
    saved_path: String,
    download_url: String,
    photo: PhotoRecord,
}

async fn api_write_metadata_copy(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(patch): Json<MetadataPatch>,
) -> impl IntoResponse {
    let src_path = state
        .photos
        .read()
        .ok()
        .and_then(|g| g.iter().find(|p| p.id == id).map(|p| p.disk_path.clone()));

    let Some(src_path) = src_path else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Không tìm thấy ảnh trong phiên hiện tại." })),
        )
            .into_response();
    };

    let dst_path = match write_metadata_copy(&src_path, &patch) {
        Ok(p) => p,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": err })),
            )
                .into_response();
        }
    };

    let new_id = format!(
        "edited-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let record = read_photo_record(&new_id, &dst_path);
    let saved_path = dst_path.to_string_lossy().into_owned();
    let download_url = format!("/api/photo/{new_id}/download");

    if let Ok(mut guard) = state.photos.write() {
        guard.push(IndexedPhoto {
            id: new_id,
            disk_path: dst_path,
            record: record.clone(),
        });
    }

    Json(WriteMetadataResponse {
        saved_path,
        download_url,
        photo: record,
    })
    .into_response()
}

#[derive(Deserialize)]
struct StripBatchRequest {
    ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StripBatchItem {
    source_id: String,
    saved_path: String,
    download_url: String,
    photo: PhotoRecord,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StripBatchError {
    source_id: String,
    error: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StripBatchResponse {
    count: usize,
    failed: usize,
    items: Vec<StripBatchItem>,
    errors: Vec<StripBatchError>,
}

fn next_unique_id(prefix: &str) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = COPY_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{ms}-{seq}")
}

async fn api_strip_metadata_copy(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let src_path = state
        .photos
        .read()
        .ok()
        .and_then(|g| g.iter().find(|p| p.id == id).map(|p| p.disk_path.clone()));

    let Some(src_path) = src_path else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Không tìm thấy ảnh trong phiên hiện tại." })),
        )
            .into_response();
    };

    let dst_path = match strip_metadata_copy(&src_path) {
        Ok(p) => p,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": err })),
            )
                .into_response();
        }
    };

    let new_id = next_unique_id("clean");
    let record = read_photo_record(&new_id, &dst_path);
    let saved_path = dst_path.to_string_lossy().into_owned();
    let download_url = format!("/api/photo/{new_id}/download");

    if let Ok(mut guard) = state.photos.write() {
        guard.push(IndexedPhoto {
            id: new_id,
            disk_path: dst_path,
            record: record.clone(),
        });
    }

    Json(WriteMetadataResponse {
        saved_path,
        download_url,
        photo: record,
    })
    .into_response()
}

async fn api_strip_metadata_batch(
    State(state): State<AppState>,
    Json(payload): Json<StripBatchRequest>,
) -> impl IntoResponse {
    if payload.ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Chưa chọn ảnh nào để xóa EXIF." })),
        )
            .into_response();
    }

    let targets: Vec<(String, Option<PathBuf>)> = state
        .photos
        .read()
        .map(|g| {
            payload
                .ids
                .iter()
                .map(|id| {
                    let found = g.iter().find(|p| &p.id == id).map(|p| p.disk_path.clone());
                    (id.clone(), found)
                })
                .collect()
        })
        .unwrap_or_default();

    let mut items = Vec::new();
    let mut errors = Vec::new();
    let mut new_indexed = Vec::new();

    for (source_id, path_opt) in targets {
        let Some(src_path) = path_opt else {
            errors.push(StripBatchError {
                source_id,
                error: "Không tìm thấy ảnh trong phiên hiện tại.".to_string(),
            });
            continue;
        };

        match strip_metadata_copy(&src_path) {
            Ok(dst_path) => {
                let new_id = next_unique_id("clean");
                let record = read_photo_record(&new_id, &dst_path);
                let saved_path = dst_path.to_string_lossy().into_owned();
                let download_url = format!("/api/photo/{new_id}/download");

                new_indexed.push(IndexedPhoto {
                    id: new_id,
                    disk_path: dst_path,
                    record: record.clone(),
                });
                items.push(StripBatchItem {
                    source_id,
                    saved_path,
                    download_url,
                    photo: record,
                });
            }
            Err(err) => {
                errors.push(StripBatchError {
                    source_id,
                    error: err,
                });
            }
        }
    }

    if !new_indexed.is_empty() {
        if let Ok(mut guard) = state.photos.write() {
            guard.extend(new_indexed);
        }
    }

    Json(StripBatchResponse {
        count: items.len(),
        failed: errors.len(),
        items,
        errors,
    })
    .into_response()
}

async fn api_download_photo(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let found = state.photos.read().ok().and_then(|g| {
        g.iter()
            .find(|p| p.id == id)
            .map(|p| (p.disk_path.clone(), p.record.filename.clone()))
    });

    let Some((path, filename)) = found else {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    };

    let Ok(bytes) = fs::read(&path) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Không thể đọc tệp ảnh").into_response();
    };

    let mime = guess_mime(&path);
    let disp = format!("attachment; filename="{}"", filename.replace('"', ""));
    (
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_DISPOSITION, disp),
        ],
        bytes,
    )
        .into_response()
}

async fn api_geojson(State(state): State<AppState>) -> impl IntoResponse {
    let features: Vec<Feature> = state
        .photos
        .read()
        .map(|g| {
            g.iter()
                .filter_map(|p| record_to_feature(&p.record))
                .collect()
        })
        .unwrap_or_default();

    Json(FeatureCollection {
        kind: "FeatureCollection",
        features,
    })
}

fn run_cli(cli_args: &[String]) {
    let input_arg = &cli_args[0];
    let mut output_arg: Option<&str> = None;
    let mut ocr_query: Option<String> = None;

    let mut i = 1;
    while i < cli_args.len() {
        if cli_args[i] == "--ocr" && i + 1 < cli_args.len() {
            ocr_query = Some(cli_args[i + 1].clone());
            i += 2;
        } else if output_arg.is_none() && !cli_args[i].starts_with("--") {
            output_arg = Some(cli_args[i].as_str());
            i += 1;
        } else {
            i += 1;
        }
    }

    let input_path = Path::new(input_arg);
    if !input_path.exists() {
        eprintln!("Lỗi: Không tìm thấy đường dẫn '{}'", input_path.display());
        process::exit(1);
    }

    let cache_path = Path::new(OCR_CACHE_FILE);
    let mut cache = OcrCache::load(cache_path);
    let mut cache_dirty = false;
    let needle = ocr_query.as_deref().map(normalize_vi);

    let image_paths = collect_image_paths(input_path);
    let total_files = image_paths.len();
    let mut features = Vec::new();
    let mut skipped = 0usize;

    for (idx, path) in image_paths.iter().enumerate() {
        let mut rec = read_photo_record(&format!("cli-{idx}"), path);

        if let Some(ref q) = needle {
            let text = if let Some(cached) = cache.get_valid(path) {
                cached
            } else {
                match ocr::recognize_text(path) {
                    Ok(t) => {
                        cache.insert(path, t.clone());
                        cache_dirty = true;
                        t
                    }
                    Err(_) => String::new(),
                }
            };
            if !normalize_vi(&text).contains(q) {
                skipped += 1;
                continue;
            }
            rec.ocr_text = if text.is_empty() { None } else { Some(text) };
        }

        match record_to_feature(&rec) {
            Some(f) => features.push(f),
            None => skipped += 1,
        }
    }

    if cache_dirty {
        let _ = cache.save(cache_path);
    }

    let collection = FeatureCollection {
        kind: "FeatureCollection",
        features,
    };

    let json = serde_json::to_string_pretty(&collection).expect("Lỗi serialize GeoJSON");
    if let Some(output_path) = output_arg {
        fs::write(output_path, &json).expect("Lỗi ghi tệp GeoJSON");
        eprintln!(
            "Đã xuất {} điểm GPS vào '{}' (quét {} ảnh, bỏ qua {} ảnh).",
            collection.features.len(),
            output_path,
            total_files,
            skipped
        );
    } else {
        println!("{json}");
        eprintln!(
            "Đã xuất {} điểm GPS (quét {} ảnh, bỏ qua {} ảnh).",
            collection.features.len(),
            total_files,
            skipped
        );
    }
}

fn record_to_feature(rec: &PhotoRecord) -> Option<Feature> {
    let (lat, lon) = (rec.lat?, rec.lon?);
    let mut coordinates = vec![lon, lat];
    if let Some(alt) = rec.altitude {
        coordinates.push(alt);
    }
    Some(Feature {
        kind: "Feature",
        geometry: Geometry {
            kind: "Point",
            coordinates,
        },
        properties: Properties {
            filename: rec.filename.clone(),
            path: rec.path.clone(),
            datetime: rec.datetime.clone(),
            camera_make: rec.camera_make.clone(),
            camera_model: rec.camera_model.clone(),
            altitude: rec.altitude,
            ocr_text: rec.ocr_text.clone(),
        },
    })
}

fn collect_image_paths(input: &Path) -> Vec<PathBuf> {
    if input.is_file() {
        return vec![input.to_path_buf()];
    }

    WalkDir::new(input)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| is_supported_image(p))
        .collect()
}

fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| SUPPORTED_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn guess_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("tif" | "tiff") => "image/tiff",
        Some("avif") => "image/avif",
        Some("heic" | "heif") => "image/heic",
        _ => "image/jpeg",
    }
}

fn read_photo_record(id: &str, path: &Path) -> PhotoRecord {
    let size_bytes = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut rec = PhotoRecord {
        id: id.to_string(),
        filename,
        path: path.to_string_lossy().into_owned(),
        size_bytes,
        thumb_url: format!("/api/photo/{id}"),
        lat: None,
        lon: None,
        altitude: None,
        datetime: None,
        camera_make: None,
        camera_model: None,
        focal_length: None,
        f_number: None,
        iso: None,
        ocr_text: None,
        ocr_status: "pending".to_string(),
    };

    let Ok(file) = File::open(path) else {
        return rec;
    };
    let mut reader = BufReader::new(file);
    let Ok(exif) = Reader::new().read_from_container(&mut reader) else {
        return rec;
    };

    if let (Some(lat_dms), Some(lon_dms)) = (
        extract_dms(&exif, Tag::GPSLatitude),
        extract_dms(&exif, Tag::GPSLongitude),
    ) {
        let lat_ref = extract_ascii_ref(&exif, Tag::GPSLatitudeRef).unwrap_or('N');
        let lon_ref = extract_ascii_ref(&exif, Tag::GPSLongitudeRef).unwrap_or('E');
        if let (Some(lat), Some(lon)) = (
            dms_to_decimal(lat_dms, lat_ref == 'S'),
            dms_to_decimal(lon_dms, lon_ref == 'W'),
        ) {
            if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
                rec.lat = Some(lat);
                rec.lon = Some(lon);
            }
        }
    }

    rec.altitude = extract_altitude(&exif);
    rec.datetime = extract_string(&exif, Tag::DateTimeOriginal)
        .or_else(|| extract_string(&exif, Tag::DateTime));
    rec.camera_make = extract_string(&exif, Tag::Make);
    rec.camera_model = extract_string(&exif, Tag::Model);
    rec.focal_length = extract_string(&exif, Tag::FocalLength);
    rec.f_number = extract_string(&exif, Tag::FNumber);
    rec.iso = exif
        .get_field(Tag::PhotographicSensitivity, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0));

    rec
}

fn rational_to_f64(r: &Rational) -> Option<f64> {
    if r.denom == 0 {
        None
    } else {
        Some(r.num as f64 / r.denom as f64)
    }
}

fn dms_to_decimal(dms: &[Rational], is_negative: bool) -> Option<f64> {
    if dms.len() < 3 {
        return None;
    }
    let degrees = rational_to_f64(&dms[0])?;
    let minutes = rational_to_f64(&dms[1])?;
    let seconds = rational_to_f64(&dms[2])?;
    let decimal = degrees + (minutes / 60.0) + (seconds / 3600.0);
    Some(if is_negative { -decimal } else { decimal })
}

fn extract_dms(exif: &Exif, tag: Tag) -> Option<&[Rational]> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::Rational(v) if v.len() >= 3 => Some(v.as_slice()),
        _ => None,
    }
}

fn extract_ascii_ref(exif: &Exif, tag: Tag) -> Option<char> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    match &field.value {
        Value::Ascii(vecs) => vecs
            .first()
            .and_then(|v| v.first())
            .map(|&b| (b as char).to_ascii_uppercase()),
        _ => None,
    }
}

fn extract_altitude(exif: &Exif) -> Option<f64> {
    let alt_field = exif.get_field(Tag::GPSAltitude, In::PRIMARY)?;
    let alt = match &alt_field.value {
        Value::Rational(v) => v.first().and_then(rational_to_f64),
        _ => None,
    }?;

    let below_sea_level = exif
        .get_field(Tag::GPSAltitudeRef, In::PRIMARY)
        .and_then(|f| match &f.value {
            Value::Byte(v) => v.first().copied(),
            _ => None,
        })
        == Some(1);

    Some(if below_sea_level { -alt } else { alt })
}

fn extract_string(exif: &Exif, tag: Tag) -> Option<String> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    let cleaned = field.display_value().to_string();
    let trimmed = cleaned.trim().trim_matches('"').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dms_to_decimal_hanoi() {
        let lat_dms = [
            Rational { num: 21, denom: 1 },
            Rational { num: 1, denom: 1 },
            Rational { num: 44, denom: 1 },
        ];
        let lat = dms_to_decimal(&lat_dms, false).unwrap();
        assert!((lat - 21.02889).abs() < 1e-4);
    }

    #[test]
    fn test_dms_zero_denominator_returns_none() {
        let dms = [
            Rational { num: 10, denom: 0 },
            Rational { num: 0, denom: 1 },
            Rational { num: 0, denom: 1 },
        ];
        assert!(dms_to_decimal(&dms, false).is_none());
    }
}
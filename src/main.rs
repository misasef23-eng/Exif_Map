use axum::{
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use exif::{Exif, In, Rational, Reader, Tag, Value};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Arc, RwLock};
use walkdir::WalkDir;

const INDEX_HTML: &str = include_str!("../web/index.html");

const SUPPORTED_EXTS: &[&str] = &[
    "jpg", "jpeg", "tif", "tiff", "heic", "heif", "png", "webp", "avif",
];

#[derive(Clone, Default)]
struct AppState {
    photos: Arc<RwLock<Vec<IndexedPhoto>>>,
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
}

#[derive(Deserialize)]
struct ScanRequest {
    folder: String,
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
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();

    // Chế độ CLI nếu truyền đối số đường dẫn (không bắt đầu bằng --serve)
    if args.len() >= 2 && args[1] != "--serve" && args[1] != "-s" {
        if args[1] == "-h" || args[1] == "--help" {
            eprintln!("Exif_Map — Quản lý ảnh EXIF & xuất GeoJSON (chỉ đọc)");
            eprintln!("  Chế độ Web UI: {} [--serve [port]]", args[0]);
            eprintln!("  Chế độ CLI:    {} <file_hoac_thu_muc> [output.geojson]", args[0]);
            process::exit(0);
        }
        run_cli(&args[1], args.get(2).map(String::as_str));
        return;
    }

    let port: u16 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3030);

    let state = AppState::default();
    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/scan", post(api_scan_folder))
        .route("/api/photos", get(api_list_photos))
        .route("/api/photo/{id}", get(api_serve_photo))
        .route("/api/geojson", get(api_geojson))
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Không thể mở cổng local server");

    eprintln!("Exif_Map Web UI đang chạy (chỉ đọc) tại: http://{addr}");
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

    let paths = collect_image_paths(&input);
    let mut indexed = Vec::with_capacity(paths.len());

    for (idx, path) in paths.into_iter().enumerate() {
        let id = format!("local-{idx}");
        let record = read_photo_record(&id, &path);
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

    Json(records).into_response()
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

fn run_cli(input_arg: &str, output_arg: Option<&str>) {
    let input_path = Path::new(input_arg);
    if !input_path.exists() {
        eprintln!("Lỗi: Không tìm thấy đường dẫn '{}'", input_path.display());
        process::exit(1);
    }

    let image_paths = collect_image_paths(input_path);
    let total_files = image_paths.len();
    let mut features = Vec::new();
    let mut skipped = 0usize;

    for (idx, path) in image_paths.iter().enumerate() {
        let rec = read_photo_record(&format!("cli-{idx}"), path);
        match record_to_feature(&rec) {
            Some(f) => features.push(f),
            None => skipped += 1,
        }
    }

    let collection = FeatureCollection {
        kind: "FeatureCollection",
        features,
    };

    let json = serde_json::to_string_pretty(&collection).expect("Lỗi serialize GeoJSON");
    if let Some(output_path) = output_arg {
        fs::write(output_path, &json).expect("Lỗi ghi tệp GeoJSON");
        eprintln!(
            "Đã xuất {} điểm GPS vào '{}' (quét {} ảnh, bỏ qua {} ảnh không có GPS).",
            collection.features.len(),
            output_path,
            total_files,
            skipped
        );
    } else {
        println!("{json}");
        eprintln!(
            "Đã xuất {} điểm GPS (quét {} ảnh, bỏ qua {} ảnh không có GPS).",
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
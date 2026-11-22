use exif::{Exif, In, Rational, Reader, Tag, Value};
use serde::Serialize;
use std::env;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process;
use walkdir::WalkDir;

const SUPPORTED_EXTS: &[&str] = &[
    "jpg", "jpeg", "tif", "tiff", "heic", "heif", "png", "webp", "avif",
];

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

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args.len() > 3 || args[1] == "-h" || args[1] == "--help" {
        eprintln!("Cách dùng: {} <file_anh_hoac_thu_muc> [output.geojson]", args[0]);
        eprintln!("  - Nếu bỏ trống output.geojson, kết quả sẽ in ra stdout.");
        process::exit(1);
    }

    let input_path = Path::new(&args[1]);
    if !input_path.exists() {
        eprintln!("Lỗi: Không tìm thấy đường dẫn '{}'", input_path.display());
        process::exit(1);
    }

    let image_paths = collect_image_paths(input_path);
    let total_files = image_paths.len();
    let mut features = Vec::new();
    let mut skipped = 0usize;

    for path in image_paths {
        match extract_feature(&path) {
            Some(feature) => features.push(feature),
            None => skipped += 1,
        }
    }

    let collection = FeatureCollection {
        kind: "FeatureCollection",
        features,
    };

    let json = match serde_json::to_string_pretty(&collection) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("Lỗi serialize GeoJSON: {err}");
            process::exit(1);
        }
    };

    if let Some(output_path) = args.get(2) {
        if let Err(err) = fs::write(output_path, &json) {
            eprintln!("Lỗi ghi file '{output_path}': {err}");
            process::exit(1);
        }
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

/// Gom danh sách ảnh từ một file đơn lẻ hoặc duyệt đệ quy một thư mục.
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
        .map(|ext| {
            let lower = ext.to_ascii_lowercase();
            SUPPORTED_EXTS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

/// Đọc EXIF của một ảnh và chuyển thành GeoJSON Feature nếu có tọa độ GPS hợp lệ.
fn extract_feature(path: &Path) -> Option<Feature> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let exif = Reader::new().read_from_container(&mut reader).ok()?;

    let lat_dms = extract_dms(&exif, Tag::GPSLatitude)?;
    let lon_dms = extract_dms(&exif, Tag::GPSLongitude)?;
    let lat_ref = extract_ascii_ref(&exif, Tag::GPSLatitudeRef).unwrap_or('N');
    let lon_ref = extract_ascii_ref(&exif, Tag::GPSLongitudeRef).unwrap_or('E');

    let lat = dms_to_decimal(lat_dms, lat_ref == 'S')?;
    let lon = dms_to_decimal(lon_dms, lon_ref == 'W')?;

    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }

    let altitude = extract_altitude(&exif);
    let mut coordinates = vec![lon, lat];
    if let Some(alt) = altitude {
        coordinates.push(alt);
    }

    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let datetime = extract_string(&exif, Tag::DateTimeOriginal)
        .or_else(|| extract_string(&exif, Tag::DateTime));

    Some(Feature {
        kind: "Feature",
        geometry: Geometry {
            kind: "Point",
            coordinates,
        },
        properties: Properties {
            filename,
            path: path.to_string_lossy().into_owned(),
            datetime,
            camera_make: extract_string(&exif, Tag::Make),
            camera_model: extract_string(&exif, Tag::Model),
            altitude,
        },
    })
}

fn rational_to_f64(r: &Rational) -> Option<f64> {
    if r.denom == 0 {
        None
    } else {
        Some(r.num as f64 / r.denom as f64)
    }
}

/// Quy đổi tọa độ Độ-Phút-Giây (DMS) sang độ thập phân (Decimal Degrees).
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

    // GPSAltitudeRef: 0 = trên mực nước biển, 1 = dưới mực nước biển
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
        // Hà Nội: 21°01'42.6"N, 105°51'15.1"E
        let lat_dms = [
            Rational { num: 21, denom: 1 },
            Rational { num: 1, denom: 1 },
            Rational { num: 426, denom: 10 },
        ];
        let lat = dms_to_decimal(&lat_dms, false).unwrap();
        assert!((lat - 21.0285).abs() < 1e-4);
    }

    #[test]
    fn test_dms_to_decimal_negative_hemisphere() {
        let dms = [
            Rational { num: 33, denom: 1 },
            Rational { num: 52, denom: 1 },
            Rational { num: 0, denom: 1 },
        ];
        let lat = dms_to_decimal(&dms, true).unwrap();
        assert!((lat - (-33.8666667)).abs() < 1e-5);
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
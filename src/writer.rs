use little_exif::exif_tag::ExifTag;
use little_exif::metadata::Metadata;
use little_exif::rational::uR64;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataPatch {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude: Option<f64>,
    pub datetime: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    #[serde(default)]
    pub output_name: Option<String>,
}

/// Tạo bản sao mới trong thư mục con `exif_map_edited/` và ghi EXIF bằng `little_exif`.
/// File gốc luôn được giữ nguyên 100%, không bao giờ bị ghi đè.
pub fn write_metadata_copy(src_path: &Path, patch: &MetadataPatch) -> Result<PathBuf, String> {
    if !src_path.exists() {
        return Err("Không tìm thấy tệp ảnh gốc.".to_string());
    }

    let dst_path = next_copy_path_in_dir(src_path, "exif_map_edited", "exif", patch.output_name.as_deref())?;
    if let Some(parent) = dst_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Không thể tạo thư mục lưu bản sao: {e}"))?;
    }

    fs::copy(src_path, &dst_path).map_err(|e| format!("Không thể sao chép tệp ảnh gốc: {e}"))?;

    // Đọc EXIF hiện có để giữ nguyên các thẻ không sửa (ống kính, ISO, tốc độ màn trập...)
    let mut metadata = Metadata::new_from_path(&dst_path).unwrap_or_else(|_| Metadata::new());

    if let (Some(lat), Some(lon)) = (patch.lat, patch.lon) {
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            let _ = fs::remove_file(&dst_path);
            return Err("Tọa độ GPS không hợp lệ (lat: -90..90, lon: -180..180).".to_string());
        }
        let lat_ref = if lat < 0.0 { "S" } else { "N" };
        let lon_ref = if lon < 0.0 { "W" } else { "E" };
        metadata.set_tag(ExifTag::GPSLatitudeRef(lat_ref.to_string()));
        metadata.set_tag(ExifTag::GPSLatitude(decimal_to_dms_ur64(lat)));
        metadata.set_tag(ExifTag::GPSLongitudeRef(lon_ref.to_string()));
        metadata.set_tag(ExifTag::GPSLongitude(decimal_to_dms_ur64(lon)));
    }

    if let Some(alt) = patch.altitude {
        if alt.is_finite() {
            let alt_ref: u8 = if alt < 0.0 { 1 } else { 0 };
            let scaled = (alt.abs() * 100.0).round() as u32;
            metadata.set_tag(ExifTag::GPSAltitudeRef(vec![alt_ref]));
            metadata.set_tag(ExifTag::GPSAltitude(vec![uR64 {
                nominator: scaled,
                denominator: 100,
            }]));
        }
    }

    if let Some(ref dt_raw) = patch.datetime {
        if let Some(exif_dt) = normalize_exif_datetime(dt_raw) {
            metadata.set_tag(ExifTag::DateTimeOriginal(exif_dt));
        }
    }

    if let Some(ref make) = patch.camera_make {
        let trimmed = make.trim();
        if !trimmed.is_empty() {
            metadata.set_tag(ExifTag::Make(trimmed.to_string()));
        }
    }

    if let Some(ref model) = patch.camera_model {
        let trimmed = model.trim();
        if !trimmed.is_empty() {
            metadata.set_tag(ExifTag::Model(trimmed.to_string()));
        }
    }

    if let Err(err) = metadata.write_to_file(&dst_path) {
        let _ = fs::remove_file(&dst_path);
        return Err(format!("Lỗi ghi EXIF bằng little_exif: {err}"));
    }

    Ok(dst_path)
}

/// Tạo bản sao sạch không còn EXIF/GPS/XMP/IPTC trong thư mục con `exif_map_clean/`.
/// File gốc luôn được giữ nguyên 100%, không bao giờ bị ghi đè.
pub fn strip_metadata_copy(src_path: &Path, custom_name: Option<&str>) -> Result<PathBuf, String> {
    if !src_path.exists() {
        return Err("Không tìm thấy tệp ảnh gốc.".to_string());
    }

    let dst_path = next_copy_path_in_dir(src_path, "exif_map_clean", "clean", custom_name)?;
    if let Some(parent) = dst_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Không thể tạo thư mục lưu bản sao sạch: {e}"))?;
    }

    let ext = src_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "jpg" | "jpeg" => {
            let bytes =
                fs::read(src_path).map_err(|e| format!("Không thể đọc tệp ảnh JPEG gốc: {e}"))?;
            let cleaned = strip_jpeg_metadata(&bytes)?;
            fs::write(&dst_path, cleaned)
                .map_err(|e| format!("Không thể ghi bản sao sạch JPEG: {e}"))?;
        }
        "png" => {
            let bytes =
                fs::read(src_path).map_err(|e| format!("Không thể đọc tệp ảnh PNG gốc: {e}"))?;
            let cleaned = strip_png_metadata(&bytes)?;
            fs::write(&dst_path, cleaned)
                .map_err(|e| format!("Không thể ghi bản sao sạch PNG: {e}"))?;
        }
        _ => {
            fs::copy(src_path, &dst_path)
                .map_err(|e| format!("Không thể sao chép tệp ảnh gốc: {e}"))?;
            if let Err(err) = Metadata::new().write_to_file(&dst_path) {
                let _ = fs::remove_file(&dst_path);
                return Err(format!("Lỗi xóa EXIF bằng little_exif: {err}"));
            }
        }
    }

    Ok(dst_path)
}

/// Xóa không nén lại (lossless) các phân đoạn metadata JPEG: APP1 (EXIF/XMP), APP12, APP13 (IPTC/Photoshop), COM.
pub fn strip_jpeg_metadata(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err("Tệp JPEG không hợp lệ (thiếu header SOI).".to_string());
    }

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..2]);
    let mut i = 2usize;

    while i < bytes.len() {
        if bytes[i] != 0xFF {
            out.extend_from_slice(&bytes[i..]);
            break;
        }

        let marker_start = i;
        while i < bytes.len() && bytes[i] == 0xFF {
            i += 1;
        }
        if i >= bytes.len() {
            out.extend_from_slice(&bytes[marker_start..]);
            break;
        }

        let marker = bytes[i];
        i += 1;

        if marker == 0xDA || marker == 0xD9 {
            out.extend_from_slice(&bytes[marker_start..]);
            break;
        }

        if marker == 0x00 || marker == 0x01 || marker == 0xD8 || (0xD0..=0xD7).contains(&marker) {
            out.extend_from_slice(&bytes[marker_start..i]);
            continue;
        }

        if i + 2 > bytes.len() {
            out.extend_from_slice(&bytes[marker_start..]);
            break;
        }

        let seg_len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
        if seg_len < 2 || i + seg_len > bytes.len() {
            out.extend_from_slice(&bytes[marker_start..]);
            break;
        }

        let seg_end = i + seg_len;
        let should_strip = matches!(marker, 0xE1 | 0xEC | 0xED | 0xFE);
        if !should_strip {
            out.extend_from_slice(&bytes[marker_start..seg_end]);
        }
        i = seg_end;
    }

    Ok(out)
}

/// Xóa không nén lại (lossless) các chunk metadata PNG: eXIf, tEXt, iTXt, zTXt, tIME.
pub fn strip_png_metadata(bytes: &[u8]) -> Result<Vec<u8>, String> {
    const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 8 || bytes[..8] != PNG_SIG {
        return Err("Tệp PNG không hợp lệ (thiếu chữ ký PNG).".to_string());
    }

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..8]);
    let mut i = 8usize;

    while i + 12 <= bytes.len() {
        let data_len =
            u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let chunk_type = &bytes[i + 4..i + 8];
        let Some(chunk_end) = i.checked_add(12).and_then(|v| v.checked_add(data_len)) else {
            out.extend_from_slice(&bytes[i..]);
            break;
        };
        if chunk_end > bytes.len() {
            out.extend_from_slice(&bytes[i..]);
            break;
        }

        let should_strip = matches!(
            chunk_type,
            b"eXIf" | b"tEXt" | b"iTXt" | b"zTXt" | b"tIME"
        );
        if !should_strip {
            out.extend_from_slice(&bytes[i..chunk_end]);
        }
        i = chunk_end;

        if chunk_type == b"IEND" {
            break;
        }
    }

    Ok(out)
}

fn sanitize_custom_stem(raw: &str, ext: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let cleaned: String = trimmed
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let lower_ext = format!(".{}", ext.to_ascii_lowercase());
    let without_ext = if cleaned.to_ascii_lowercase().ends_with(&lower_ext) {
        &cleaned[..cleaned.len() - lower_ext.len()]
    } else {
        cleaned.as_str()
    };
    let final_stem = without_ext.trim().trim_matches('.');
    if final_stem.is_empty() {
        None
    } else {
        Some(final_stem.to_string())
    }
}

/// Tạo đường dẫn bản sao mới không trùng tên trong `<thu_muc_goc>/<dir_name>/`.
fn next_copy_path_in_dir(
    src_path: &Path,
    dir_name: &str,
    suffix: &str,
    custom_name: Option<&str>,
) -> Result<PathBuf, String> {
    let parent = src_path.parent().unwrap_or_else(|| Path::new("."));
    let out_dir = parent.join(dir_name);
    let stem = src_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Tên tệp ảnh không hợp lệ.".to_string())?;
    let ext = src_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpg");

    let base_name = custom_name
        .and_then(|raw| sanitize_custom_stem(raw, ext))
        .unwrap_or_else(|| format!("{stem}_{suffix}"));

    let mut candidate = out_dir.join(format!("{base_name}.{ext}"));
    let mut counter = 2u32;
    while candidate.exists() {
        candidate = out_dir.join(format!("{base_name}_{counter}.{ext}"));
        counter += 1;
    }
    Ok(candidate)
}

/// Quy đổi độ thập phân sang 3 giá trị Rational không dấu [độ, phút, giây] cho chuẩn EXIF GPS.
pub fn decimal_to_dms_ur64(decimal: f64) -> Vec<uR64> {
    let abs = decimal.abs();
    let deg = abs.floor() as u32;
    let rem_min = (abs - deg as f64) * 60.0;
    let min = rem_min.floor() as u32;
    let sec = (rem_min - min as f64) * 60.0;
    let sec_scaled = (sec * 10_000.0).round() as u32;

    vec![
        uR64 {
            nominator: deg,
            denominator: 1,
        },
        uR64 {
            nominator: min,
            denominator: 1,
        },
        uR64 {
            nominator: sec_scaled,
            denominator: 10_000,
        },
    ]
}

/// Chuẩn hóa chuỗi ngày giờ (`YYYY-MM-DD HH:MM:SS` hoặc `YYYY-MM-DDTHH:MM`) sang chuẩn EXIF `YYYY:MM:DD HH:MM:SS`.
pub fn normalize_exif_datetime(raw: &str) -> Option<String> {
    let s = raw.trim().replace('T', " ");
    if s.len() < 10 {
        return None;
    }
    let parts: Vec<&str> = s.split_whitespace().collect();
    let date = parts[0].replace('-', ":");
    if date.len() != 10 {
        return None;
    }
    let time = match parts.get(1) {
        Some(t) if t.len() == 5 => format!("{t}:00"),
        Some(t) if t.len() >= 8 => t[..8].to_string(),
        _ => "00:00:00".to_string(),
    };
    Some(format!("{date} {time}"))
}


/// Sao chép trực tiếp tệp ảnh (giữ nguyên 100% metadata EXIF/GPS) vào clipboard hệ điều hành
/// để người dùng có thể Cmd+V / Ctrl+V trực tiếp vào Finder, Explorer, chat hoặc app khác.
pub fn copy_file_to_clipboard(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err("Không tìm thấy tệp ảnh để sao chép vào clipboard.".to_string());
    }
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    #[cfg(target_os = "macos")]
    {
        let escaped = canonical
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let script = format!("set the clipboard to (POSIX file \"{escaped}\")");
        let output = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| format!("Không thể chạy osascript: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Lỗi sao chép clipboard macOS: {}", stderr.trim()));
        }
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let raw = canonical.to_string_lossy();
        let clean = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
        let escaped = clean.replace('\'', "''");
        let ps_cmd = format!("Set-Clipboard -LiteralPath '{escaped}'");
        let output = Command::new("powershell")
            .args(["-NoProfile", "-STA", "-Command", &ps_cmd])
            .output()
            .map_err(|e| format!("Không thể chạy PowerShell Set-Clipboard: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Lỗi sao chép clipboard Windows: {}", stderr.trim()));
        }
        return Ok(());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        use std::io::Write;
        use std::process::Stdio;
        let uri = format!("file://{}\n", canonical.to_string_lossy());
        let candidates: &[(&str, &[&str])] = &[
            ("wl-copy", &["--type", "text/uri-list"]),
            ("xclip", &["-selection", "clipboard", "-t", "text/uri-list"]),
        ];
        for (bin, args) in candidates {
            if let Ok(mut child) = Command::new(bin)
                .args(*args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(uri.as_bytes());
                }
                if let Ok(status) = child.wait() {
                    if status.success() {
                        return Ok(());
                    }
                }
            }
        }
        Err("Cần cài đặt `wl-copy` (Wayland) hoặc `xclip` (X11) để sao chép tệp ảnh vào clipboard trên Linux.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decimal_to_dms_ur64_hanoi() {
        let dms = decimal_to_dms_ur64(21.02889);
        assert_eq!(dms[0].nominator, 21);
        assert_eq!(dms[1].nominator, 1);
        let sec = dms[2].nominator as f64 / dms[2].denominator as f64;
        assert!((sec - 44.0).abs() < 0.05);
    }

    #[test]
    fn test_normalize_exif_datetime() {
        assert_eq!(
            normalize_exif_datetime("2026-10-22T14:46"),
            Some("2026:10:22 14:46:00".to_string())
        );
        assert_eq!(
            normalize_exif_datetime("2026-10-22 14:46:46"),
            Some("2026:10:22 14:46:46".to_string())
        );
    }

    #[test]
    fn test_strip_jpeg_metadata_removes_app1_and_com() {
        // SOI + APP0 (giữ) + APP1 EXIF (xóa) + COM (xóa) + SOS + scan data + EOI
        let mut jpeg = Vec::new();
        jpeg.extend_from_slice(&[0xFF, 0xD8]);
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x06, b'J', b'F', b'I', b'F']);
        jpeg.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x08, b'E', b'x', b'i', b'f', 0, 0]);
        jpeg.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x05, b'a', b'b', b'c']);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x04, 0x11, 0x22, 0xFF, 0xD9]);

        let cleaned = strip_jpeg_metadata(&jpeg).unwrap();
        let expected = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x06, b'J', b'F', b'I', b'F', 0xFF, 0xDA, 0x00, 0x04,
            0x11, 0x22, 0xFF, 0xD9,
        ];
        assert_eq!(cleaned, expected);
    }

    #[test]
    fn test_strip_png_metadata_removes_exif_and_text() {
        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // IHDR (data_len = 0, giả lập gọn)
        png.extend_from_slice(&[0, 0, 0, 0, b'I', b'H', b'D', b'R', 1, 2, 3, 4]);
        // eXIf (xóa)
        png.extend_from_slice(&[0, 0, 0, 2, b'e', b'X', b'I', b'f', 0xAA, 0xBB, 5, 6, 7, 8]);
        // tEXt (xóa)
        png.extend_from_slice(&[0, 0, 0, 2, b't', b'E', b'X', b't', b'h', b'i', 9, 10, 11, 12]);
        // IEND (giữ)
        png.extend_from_slice(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0, 0, 0, 0]);

        let cleaned = strip_png_metadata(&png).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        expected.extend_from_slice(&[0, 0, 0, 0, b'I', b'H', b'D', b'R', 1, 2, 3, 4]);
        expected.extend_from_slice(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0, 0, 0, 0]);
        assert_eq!(cleaned, expected);
    }
}

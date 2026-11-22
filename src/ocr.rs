use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrCacheEntry {
    pub size_bytes: u64,
    pub modified_secs: u64,
    pub text: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OcrCache {
    pub entries: HashMap<String, OcrCacheEntry>,
}

impl OcrCache {
    pub fn load(cache_path: &Path) -> Self {
        let Ok(raw) = fs::read_to_string(cache_path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save(&self, cache_path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Lỗi serialize OCR cache: {e}"))?;
        fs::write(cache_path, json).map_err(|e| format!("Lỗi ghi OCR cache: {e}"))
    }

    /// Trả về văn bản OCR đã lưu nếu kích thước và thời gian sửa đổi file không đổi.
    pub fn get_valid(&self, image_path: &Path) -> Option<String> {
        let key = canonical_key(image_path);
        let (size_bytes, modified_secs) = file_signature(image_path)?;
        let entry = self.entries.get(&key)?;
        if entry.size_bytes == size_bytes && entry.modified_secs == modified_secs {
            Some(entry.text.clone())
        } else {
            None
        }
    }

    pub fn insert(&mut self, image_path: &Path, text: String) {
        if let Some((size_bytes, modified_secs)) = file_signature(image_path) {
            let key = canonical_key(image_path);
            self.entries.insert(
                key,
                OcrCacheEntry {
                    size_bytes,
                    modified_secs,
                    text,
                },
            );
        }
    }
}

fn canonical_key(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn file_signature(path: &Path) -> Option<(u64, u64)> {
    let meta = fs::metadata(path).ok()?;
    let modified_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((meta.len(), modified_secs))
}

/// Nhận diện chữ trong ảnh bằng OCR gốc của hệ điều hành (chỉ đọc).
/// - macOS: Apple Vision (VNRecognizeTextRequest, hỗ trợ vi-VN & en-US)
/// - Windows: Windows.Media.Ocr (WinRT có sẵn trên Windows 10/11)
/// - Linux: Tesseract CLI (vie+eng)
pub fn recognize_text(image_path: &Path) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        recognize_macos_vision(image_path)
    }

    #[cfg(target_os = "windows")]
    {
        recognize_windows_ocr(image_path)
    }

    #[cfg(target_os = "linux")]
    {
        recognize_linux_tesseract(image_path)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = image_path;
        Err("Hệ điều hành hiện tại chưa được hỗ trợ OCR gốc.".to_string())
    }
}

#[cfg(target_os = "macos")]
fn recognize_macos_vision(image_path: &Path) -> Result<String, String> {
    const SWIFT_VISION_SCRIPT: &str = r#"
import Foundation
import Vision
import ImageIO

guard CommandLine.arguments.count > 1 else { exit(1) }
let path = CommandLine.arguments[1]
let url = URL(fileURLWithPath: path)
guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
      let cgImage = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
    fputs("Cannot decode image\n", stderr)
    exit(2)
}

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true
request.recognitionLanguages = ["vi-VN", "en-US"]

let handler = VNImageRequestHandler(cgImage: cgImage, options: [:])
do {
    try handler.perform([request])
    let lines = (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }
    print(lines.joined(separator: "\n"))
} catch {
    fputs("\(error)\n", stderr)
    exit(3)
}
"#;

    let output = Command::new("/usr/bin/swift")
        .arg("-e")
        .arg(SWIFT_VISION_SCRIPT)
        .arg(image_path)
        .output()
        .map_err(|e| format!("Không thể chạy macOS Vision qua swift: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "macOS Vision trả về lỗi khi nhận diện ảnh.".to_string()
        } else {
            err
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "windows")]
fn recognize_windows_ocr(image_path: &Path) -> Result<String, String> {
    let ps_script = r#"
param([string]$ImgPath)
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
$null = [Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType=WindowsRuntime]
$asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
})[0]
function Await($WinRtTask, $ResultType) {
    $asTask = $asTaskGeneric.MakeGenericMethod($ResultType)
    $netTask = $asTask.Invoke($null, @($WinRtTask))
    $netTask.Wait(-1) | Out-Null
    return $netTask.Result
}
$fullPath = (Resolve-Path $ImgPath).Path
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($fullPath)) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Write-Output $result.Text
"#;

    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            ps_script,
            "-ImgPath",
            &image_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("Không thể chạy Windows OCR qua PowerShell: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "Windows OCR trả về lỗi khi nhận diện ảnh.".to_string()
        } else {
            err
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "linux")]
fn recognize_linux_tesseract(image_path: &Path) -> Result<String, String> {
    let output = Command::new("tesseract")
        .arg(image_path)
        .arg("stdout")
        .args(["-l", "vie+eng", "--psm", "3"])
        .output()
        .map_err(|e| format!("Không thể chạy tesseract (hãy cài tesseract-ocr): {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "Tesseract trả về lỗi khi nhận diện ảnh.".to_string()
        } else {
            err
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Chuẩn hóa chuỗi tiếng Việt về chữ thường không dấu để tìm kiếm OCR linh hoạt.
pub fn normalize_vi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.to_lowercase().chars() {
        let mapped = match ch {
            'à' | 'á' | 'ạ' | 'ả' | 'ã' | 'â' | 'ầ' | 'ấ' | 'ậ' | 'ẩ' | 'ẫ' | 'ă' | 'ằ' | 'ắ'
            | 'ặ' | 'ẳ' | 'ẵ' => 'a',
            'è' | 'é' | 'ẹ' | 'ẻ' | 'ẽ' | 'ê' | 'ề' | 'ế' | 'ệ' | 'ể' | 'ễ' => 'e',
            'ì' | 'í' | 'ị' | 'ỉ' | 'ĩ' => 'i',
            'ò' | 'ó' | 'ọ' | 'ỏ' | 'õ' | 'ô' | 'ồ' | 'ố' | 'ộ' | 'ổ' | 'ỗ' | 'ơ' | 'ờ' | 'ớ'
            | 'ợ' | 'ở' | 'ỡ' => 'o',
            'ù' | 'ú' | 'ụ' | 'ủ' | 'ũ' | 'ư' | 'ừ' | 'ứ' | 'ự' | 'ử' | 'ữ' => 'u',
            'ỳ' | 'ý' | 'ỵ' | 'ỷ' | 'ỹ' => 'y',
            'đ' => 'd',
            other => other,
        };
        out.push(mapped);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_vi_matches_without_accents() {
        assert_eq!(normalize_vi("Nội Dung A — Đà Nẵng"), "noi dung a — da nang");
        assert!(normalize_vi("Tháp Rùa Hồ Hoàn Kiếm").contains(&normalize_vi("thap rua")));
    }
}

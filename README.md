# Exif_Map

Công cụ dòng lệnh viết bằng Rust giúp đọc EXIF metadata từ ảnh, trích xuất tọa độ GPS và xuất ra định dạng **GeoJSON (RFC 7946)** để hiển thị trực tiếp trên bản đồ.

## Tính năng

- **Đa định dạng ảnh:** Hỗ trợ JPEG, TIFF, HEIC/HEIF, PNG, WebP, AVIF (thông qua [`kamadak-exif`](https://docs.rs/kamadak-exif)).
- **Quét linh hoạt:** Xử lý 1 tệp ảnh đơn lẻ hoặc tự động duyệt đệ quy toàn bộ thư mục con.
- **Chuẩn GeoJSON:** Tự động quy đổi tọa độ Độ-Phút-Giây (DMS) sang độ thập phân theo đúng thứ tự `[kinh_độ, vĩ_độ]` (và kèm `độ_cao` nếu ảnh có dữ liệu).
- **Kèm metadata hữu ích:** Lưu tên tệp, đường dẫn, thời gian chụp (`DateTimeOriginal`), hãng và mẫu máy ảnh (`Make`, `Model`).
- **An toàn:** Tự động bỏ qua ảnh không có EXIF/GPS hoặc tọa độ không hợp lệ và in thống kê ra `stderr`.

## Yêu cầu

- Rust & Cargo (phiên bản `1.60` trở lên).

## Cấu trúc dự án

```text
Exif_Map/
├── Cargo.toml
├── README.md
└── src/
    └── main.rs
```

## Cài đặt & Sử dụng

### 1. Chạy trực tiếp bằng Cargo

Quét một thư mục ảnh và xuất ra tệp `map.geojson`:

```bash
cargo run -- ./photos map.geojson
```

Quét một ảnh đơn lẻ và in kết quả GeoJSON ra `stdout` (tiện dùng với pipe hoặc chuyển hướng):

```bash
cargo run -- ./photo.jpg > photo.geojson
```

### 2. Biên dịch bản release

```bash
cargo build --release
./target/release/Exif_Map ./photos map.geojson
```

### 3. Chạy unit test

```bash
cargo test
```

## Ví dụ đầu ra GeoJSON

```json
{
  "type": "FeatureCollection",
  "features": [
    {
      "type": "Feature",
      "geometry": {
        "type": "Point",
        "coordinates": [105.8542, 21.0285, 12.5]
      },
      "properties": {
        "filename": "hanoi.jpg",
        "path": "./photos/hanoi.jpg",
        "datetime": "2026-11-22 10:30:00",
        "camera_make": "Apple",
        "camera_model": "iPhone 15 Pro",
        "altitude": 12.5
      }
    }
  ]
}
```

## Hiển thị lên bản đồ

| Công cụ | Cách xem nhanh |
| :--- | :--- |
| **geojson.io** | Mở trình duyệt tại `geojson.io` và kéo thả trực tiếp tệp `map.geojson` vào bản đồ. |
| **QGIS** | Kéo thả tệp `map.geojson` vào cửa sổ bản đồ để thêm dưới dạng Vector Layer. |
| **Leaflet / Mapbox** | Nạp trực tiếp tệp bằng `fetch('map.geojson')` rồi truyền vào `L.geoJSON(data).addTo(map)`. |

## Lưu ý

- Các ảnh đã bị nén qua mạng xã hội hoặc ứng dụng nhắn tin (Zalo, Messenger, Facebook...) thường bị xóa sạch EXIF nên sẽ bị bỏ qua.
- Thứ tự tọa độ trong GeoJSON luôn là `[longitude, latitude]`, ngược lại với thứ tự `(latitude, longitude)` thường thấy trên Google Maps.

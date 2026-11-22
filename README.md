# Exif_Map

Ứng dụng **Rust local server ([`axum`](https://docs.rs/axum/latest/axum/)) + Web UI bản đồ tràn viền** giúp quản lý thư viện ảnh EXIF theo chế độ **chỉ đọc**, tự động gom cụm ảnh theo tọa độ GPS kiểu Apple Photos và xuất dữ liệu **GeoJSON (RFC 7946)**.

## Tính năng chính

* **Bản đồ tràn viền kiểu Apple Photos:** Hiển thị ảnh dưới dạng ghim vuông bo góc viền trắng có mũi nhọn; các ảnh gần nhau tự động gom cụm kèm bộ đếm số lượng ảnh và tách cụm mượt mà khi phóng to.
* **Quản lý file ảnh chỉ đọc (Read-only):** Ngăn kéo kính mờ thu gọn được ở cạnh trái hỗ trợ xem dạng lưới/danh sách, tìm kiếm theo tên/máy ảnh/thư mục, lọc theo trạng thái GPS (`Tất cả`, `Có GPS`, `Không GPS`, `Đã chọn`), lọc theo mẫu máy ảnh và sắp xếp theo thời gian chụp. Không bao giờ sửa đổi, di chuyển hay xóa tệp ảnh gốc.
* **Kéo thả trực tiếp:** Kéo thả từng ảnh hoặc cả thư mục (bao gồm thư mục con) vào ngăn kéo hoặc thả thẳng lên bất kỳ vị trí nào trên bản đồ.
* **Đa định dạng ảnh:** Hỗ trợ JPEG, TIFF, HEIC/HEIF, PNG, WebP, AVIF thông qua [`kamadak-exif`](https://docs.rs/kamadak-exif) (bản trình duyệt đọc trực tiếp EXIF từ JPEG/TIFF; bản Rust backend hỗ trợ đầy đủ cả 6 nhóm định dạng).
* **Tùy chỉnh bản đồ & lộ trình:** Chuyển nhanh 4 lớp nền bản đồ (`Địa hình`, `Đường phố`, `Vệ tinh`, `Tối giản`), nối lộ trình ảnh theo mốc thời gian chụp, nút thu phóng vừa khung và la bàn đặt lại góc nhìn.
* **Xuất chuẩn GeoJSON (RFC 7946):** Xem trước mã JSON, sao chép nhanh hoặc tải tệp `exif_map.geojson` theo đúng thứ tự tọa độ `[kinh_độ, vĩ_độ]` (kèm `độ_cao` nếu có).
* **Hai chế độ linh hoạt:** Chạy giao diện Web UI qua máy chủ cục bộ hoặc chạy trực tiếp bằng dòng lệnh (CLI).

## Yêu cầu hệ thống

* **Rust & Cargo:** Phiên bản `1.75` trở lên (yêu cầu của `axum 0.8`).
* **Trình duyệt hiện đại:** Chrome, Edge, Safari hoặc Firefox (cần kết nối mạng để tải lớp nền bản đồ Esri ArcGIS).

## Cấu trúc dự án

```text
Exif_Map/
├── Cargo.toml        # Khai báo package Exif_Map và các crate phụ thuộc
├── README.md         # Tài liệu hướng dẫn chi tiết
├── src/
│   └── main.rs       # Máy chủ cục bộ Axum (chỉ đọc) + chế độ CLI
└── web/
    └── index.html    # Giao diện Web UI (nhúng trực tiếp vào binary bằng include_str!)
```

## Cài đặt & Cách chạy

### 1. Chạy chế độ Web UI (mặc định)

Mở terminal trong thư mục `Exif_Map/` và chạy:

```bash
cargo run
```

Máy chủ cục bộ sẽ khởi động tại:

```text
http://127.0.0.1:3030
```

Nếu muốn đổi cổng (ví dụ cổng `8080`):

```bash
cargo run -- --serve 8080
```

### 2. Chạy chế độ dòng lệnh (CLI)

Quét một thư mục ảnh (duyệt đệ quy tất cả thư mục con) và lưu kết quả ra tệp `map.geojson`:

```bash
cargo run -- ./photos map.geojson
```

Quét một tệp ảnh đơn lẻ và in GeoJSON ra `stdout` (tiện dùng với pipe hoặc chuyển hướng):

```bash
cargo run -- ./photo.jpg > photo.geojson
```

### 3. Biên dịch bản tối ưu (Release)

```bash
cargo build --release
```

File thực thi sau khi biên dịch nằm tại `./target/release/Exif_Map` (hoặc `target\release\Exif_Map.exe` trên Windows) — chứa sẵn cả giao diện Web UI bên trong nên có thể chạy độc lập ở bất kỳ đâu:

```bash
./target/release/Exif_Map
./target/release/Exif_Map ./photos map.geojson
```

### 4. Chạy kiểm thử (Unit Tests)

```bash
cargo test
```

## Hướng dẫn sử dụng giao diện Web UI

| Khu vực / Thao tác | Cách dùng |
| :--- | :--- |
| **Nạp ảnh & thư mục** | Kéo thả ảnh/thư mục lên bất kỳ đâu trên bản đồ, hoặc bấm **Thêm ảnh** / **Thư mục** trên thanh công cụ góc trên phải. |
| **Ẩn / hiện quản lý ảnh** | Bấm nút biểu tượng ngăn kéo màu xanh cạnh logo **Exif_Map** (góc trên trái) để mở rộng bản đồ toàn màn hình. |
| **Xem cụm ảnh trên bản đồ** | Bấm vào ghim có số lượng lớn hơn `1` để mở dải ảnh phía dưới; bấm **Phóng to cụm** hoặc cuộn chuột để tách thành từng ghim riêng. |
| **Xem chi tiết EXIF** | Bấm vào thẻ ảnh trong danh sách hoặc bấm **Xem chi tiết EXIF** trên popup bản đồ để xem tọa độ thập phân, DMS, độ cao, máy ảnh, tiêu cự, khẩu độ, ISO và sao chép nhanh GPS. |
| **Lọc & chọn ảnh để xuất** | Dùng ô tìm kiếm, bộ lọc `Có GPS / Không GPS / Đã chọn`, chọn mẫu máy ảnh và tick vào các ảnh muốn xuất; bấm **GeoJSON** để xem trước hoặc **Xuất** để tải tệp `exif_map.geojson`. |
| **Điều khiển bản đồ** | Thanh trên giữa đổi lớp nền (`Địa hình`, `Đường phố`, `Vệ tinh`, `Tối giản`); cụm góc dưới phải gồm bật/tắt **Nối lộ trình**, **Vừa khung**, phóng to/thu nhỏ và nút la bàn **B**. |

## API của Rust Local Server

Khi chạy ở chế độ máy chủ cục bộ (`127.0.0.1:3030`), `Exif_Map` cung cấp các endpoint chỉ đọc sau:

| Method | Endpoint | Mô tả |
| :--- | :--- | :--- |
| `GET` | `/` | Trả về giao diện Web UI (`web/index.html`). |
| `POST` | `/api/scan` | Nhận JSON `{ "folder": "/duong/dan/thu/muc" }`, quét EXIF chỉ đọc và trả về danh sách metadata ảnh. |
| `GET` | `/api/photos` | Trả về danh sách ảnh đã quét trong phiên hiện tại. |
| `GET` | `/api/photo/{id}` | Trả về nội dung tệp ảnh theo `id` đã lập chỉ mục (chặn truy cập đường dẫn tùy ý). |
| `GET` | `/api/geojson` | Xuất trực tiếp toàn bộ ảnh có GPS trong phiên dưới dạng GeoJSON `FeatureCollection`. |

## Ví dụ đầu ra GeoJSON

```json
{
  "type": "FeatureCollection",
  "features": [
    {
      "type": "Feature",
      "geometry": {
        "type": "Point",
        "coordinates": [105.8525, 21.02889, 15.9]
      },
      "properties": {
        "filename": "hoan_kiem_hoang_hon.jpg",
        "path": "sample_album/hanoi/hoan_kiem_hoang_hon.jpg",
        "datetime": "2026-10-15 17:42:10",
        "camera_make": "NIKON CORPORATION",
        "camera_model": "Nikon Z6 II",
        "altitude": 15.9,
        "focal_length": "35 mm",
        "aperture": "f/2.8",
        "iso": 200
      }
    }
  ]
}
```

## Hiển thị GeoJSON trên công cụ khác

| Công cụ | Cách xem nhanh |
| :--- | :--- |
| **geojson.io** | Mở trình duyệt tại `geojson.io` và kéo thả trực tiếp tệp `exif_map.geojson` vào bản đồ. |
| **QGIS** | Kéo thả tệp `exif_map.geojson` vào cửa sổ bản đồ để thêm dưới dạng Vector Layer. |
| **Leaflet / Mapbox** | Nạp bằng `fetch('exif_map.geojson')` rồi truyền vào `L.geoJSON(data).addTo(map)`. |

## Lưu ý

* **An toàn dữ liệu:** Ứng dụng hoạt động hoàn toàn ở chế độ chỉ đọc; nút **Xóa DS** chỉ làm trống danh sách hiển thị trong phiên hiện tại, không tác động tới tệp gốc.
* **Ảnh bị mất EXIF:** Các ảnh gửi qua Zalo, Messenger, Facebook hoặc ứng dụng nhắn tin thường bị nén và xóa sạch EXIF nên sẽ hiển thị ở mục **Không GPS**.
* **Thứ tự tọa độ:** Chuẩn GeoJSON (RFC 7946) luôn đặt `[longitude, latitude]` (kinh độ trước, vĩ độ sau), ngược lại với thứ tự `(latitude, longitude)` thường thấy khi tìm trên Google Maps.
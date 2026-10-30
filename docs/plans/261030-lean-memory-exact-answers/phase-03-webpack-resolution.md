# Bước 3: dự án webpack / Laravel Mix / Vue CLI

## Lỗi

`drivers/javascript/driver.mjs` (`readModuleResolution`) chỉ đọc alias từ cấu hình Vite và `tsconfig`/`jsconfig`.
Trên vitalnoni-v2 (Vue 2.7, Laravel Mix): `@` khai báo trong `webpack.mix.js`, không có `jsconfig`;
`CustomPagination.vue` được 11 file import qua `@/components/...`, scout báo 0 người gọi, không cảnh báo. 222 route
import component không ghi đuôi `.vue` (webpack tự thêm), cũng không được nối.

## Làm

1. Khi có `webpack.mix.js`, `vue.config.js` hay `webpack.config.*` mà alias của nó không đọc được: cảnh báo kèm câu
   trả lời, như cảnh báo Vite hiện có. Làm trước, rẻ, chặn kiểu lỗi im lặng.
2. Đọc `resolve.alias` và `resolve.extensions` như đang làm với Vite: chạy file cấu hình trong tiến trình con ngắn,
   lấy kết quả; không chạy được thì đọc alias như được viết.

## Kiểm

`CustomPagination.vue#default` ra 11 chỗ; một route component ra dòng `import()` của router; repo Vite cho câu trả
lời giống hệt mẫu bước 0.

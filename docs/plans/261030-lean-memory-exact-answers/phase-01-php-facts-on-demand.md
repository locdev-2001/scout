# Bước 1: PHP giữ facts gọn (cổng)

## Hiện trạng (đo ở bước 0)

Driver PHP trên backend giữ ~340 MB: engine ~33 MB, autoload Composer ~35 MB, facts và index ~270 MB. Trong facts
của từng file: `sites` 91 MB (373.480 chỗ gọi), `questions` 25 MB, `scopes` 16 MB, `units` 12 MB, `mentions`
10 MB; facts class: `methods` 26,5 MB. Phần chênh (~70 MB) là index dẫn xuất, đo ở bước 1a.1.

Đọc từ đĩa mỗi câu hỏi không hợp: `magicCallers` quét mọi chỗ gọi của mọi file với mọi class có `__call` (mọi model
Eloquent), `classesWorthDescribing` và tìm method theo tên trần cũng quét toàn bộ.

## 1a: nén trong RAM

1. Đo index dẫn xuất (`pathsByName`, `constructions`, `declaredClasses`, `callersCache`, `cacheUsers`,
   `dependents`, `ownerDeps`, `sharedLists`, `scanned`) trên driver chạy thật, đọc một lần.
2. Mỗi file giữ một mảng meta nhỏ (`hash`, `answerHash`, `deps`, `classes`, `mentions`, `carried`) và một chuỗi
   igbinary chứa `byName` (chỗ gọi đã gộp theo tên, kiểu nhận giữ ở dạng chỉ số vào danh sách dùng chung),
   `questions`, `scopes`, `units`. `isset($this->fileFacts[$path])` giữ nguyên nghĩa "đã đọc".
3. Một accessor cho mỗi phần; không chỗ nào khác giải mã. Một bộ nhớ đệm nhỏ cho vài file vừa giải mã.
4. `answerHash` tính ra cùng giá trị như hôm nay.

## 1b: facts trên đĩa (chỉ khi 1a chưa đủ)

Ghi các chuỗi của 1a vào một gói trên đĩa, giữ index trong RAM, đọc khối khi cần. Cần index thay cho ba vòng quét
toàn bộ trước khi làm.

## Kiểm (cổng)

- `diff` câu trả lời của bộ 40 câu với mẫu bước 0: rỗng (bản sao và repo thật).
- Kịch bản sửa file, chạy trên build cũ và mới rồi `diff`: thêm một lời gọi vào một file, hỏi lại (phải có cạnh
  mới); xoá lời gọi, hỏi lại (phải mất).
- p95 không tăng quá 10 %; đỉnh PSS trong lúc chạy bộ câu hỏi; USS tiến trình con chờ sẵn.
- PSS driver PHP giảm; ghi các khoản còn lại theo thứ tự lớn nhỏ.
- Không đạt một điều → dừng, ghi lý do.

## Sau khi qua cổng

Đổi code driver PHP là đổi khoá cache: mọi repo PHP đọc lại một lần (~9 s câu đầu trên backend). Đưa vào `main`, bật
lại repo thật rồi mới báo xong.

## Rủi ro và lui lại

`answerHash` hay meta sai làm cache câu trả lời bị bỏ sai cách: lỗi im lặng, chốt chặn là kịch bản sửa file.
Lui lại: một commit.

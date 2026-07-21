#include "image_attachment_picker.h"

#include <flutter/standard_method_codec.h>
#include <propvarutil.h>
#include <shobjidl.h>
#include <wincodec.h>
#include <wrl/client.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <iomanip>
#include <limits>
#include <sstream>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include "utils.h"

namespace {

using Microsoft::WRL::ComPtr;

constexpr char kChannelName[] = "dev.phi.phi_client/image_attachment_picker";
constexpr int kDefaultMaxImages = 3;
constexpr int kMaxImages = 3;
constexpr UINT kMaxOutputDimension = 1600;
constexpr UINT kMinOutputDimension = 480;
constexpr size_t kMaxImageBytes = 200 * 1024;

class PickerException : public std::runtime_error {
 public:
  PickerException(std::string code, std::string message)
      : std::runtime_error(std::move(message)), code_(std::move(code)) {}

  const std::string& code() const { return code_; }

 private:
  std::string code_;
};

std::string DescribeHresult(HRESULT result) {
  std::ostringstream message;
  message << "HRESULT 0x" << std::hex << std::uppercase << std::setfill('0')
          << std::setw(8) << static_cast<uint32_t>(result);
  return message.str();
}

void CheckHresult(HRESULT result, const char* operation,
                  const char* code = "image_processing_failed") {
  if (SUCCEEDED(result)) {
    return;
  }
  throw PickerException(code, std::string(operation) + " failed (" +
                                  DescribeHresult(result) + ").");
}

int ReadMaxCount(const flutter::EncodableValue* arguments) {
  if (arguments == nullptr) {
    return kDefaultMaxImages;
  }
  const auto* map = std::get_if<flutter::EncodableMap>(arguments);
  if (map == nullptr) {
    return kDefaultMaxImages;
  }
  const auto found = map->find(flutter::EncodableValue("maxCount"));
  if (found == map->end()) {
    return kDefaultMaxImages;
  }
  if (const auto* value = std::get_if<int32_t>(&found->second)) {
    return std::clamp(static_cast<int>(*value), 1, kMaxImages);
  }
  if (const auto* value = std::get_if<int64_t>(&found->second)) {
    return static_cast<int>(std::clamp<int64_t>(*value, 1, kMaxImages));
  }
  return kDefaultMaxImages;
}

std::vector<std::wstring> PickImagePaths(HWND owner, int max_count) {
  ComPtr<IFileOpenDialog> dialog;
  CheckHresult(CoCreateInstance(CLSID_FileOpenDialog, nullptr,
                                CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&dialog)),
               "Opening the Windows file picker", "picker_unavailable");

  DWORD options = 0;
  CheckHresult(dialog->GetOptions(&options), "Reading file picker options",
               "picker_unavailable");
  CheckHresult(
      dialog->SetOptions(options | FOS_ALLOWMULTISELECT | FOS_FILEMUSTEXIST |
                         FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST),
      "Configuring the Windows file picker", "picker_unavailable");

  constexpr COMDLG_FILTERSPEC kImageFilter[] = {
      {L"Image files", L"*.jpg;*.jpeg;*.png;*.bmp;*.gif;*.tif;*.tiff"},
      {L"All files", L"*.*"},
  };
  CheckHresult(dialog->SetFileTypes(static_cast<UINT>(std::size(kImageFilter)),
                                    kImageFilter),
               "Configuring image file types", "picker_unavailable");

  const HRESULT show_result = dialog->Show(owner);
  if (show_result == HRESULT_FROM_WIN32(ERROR_CANCELLED)) {
    return {};
  }
  CheckHresult(show_result, "Showing the Windows file picker",
               "picker_unavailable");

  ComPtr<IShellItemArray> selected_items;
  CheckHresult(dialog->GetResults(&selected_items),
               "Reading selected image files", "picker_unavailable");

  DWORD item_count = 0;
  CheckHresult(selected_items->GetCount(&item_count),
               "Counting selected image files", "picker_unavailable");
  const DWORD accepted_count =
      std::min(item_count, static_cast<DWORD>(max_count));

  std::vector<std::wstring> paths;
  paths.reserve(accepted_count);
  for (DWORD index = 0; index < accepted_count; ++index) {
    ComPtr<IShellItem> item;
    CheckHresult(selected_items->GetItemAt(index, &item),
                 "Reading a selected image", "picker_unavailable");

    PWSTR raw_path = nullptr;
    CheckHresult(item->GetDisplayName(SIGDN_FILESYSPATH, &raw_path),
                 "Reading a selected image path", "picker_unavailable");
    paths.emplace_back(raw_path);
    CoTaskMemFree(raw_path);
  }
  return paths;
}

USHORT ReadExifOrientation(IWICBitmapFrameDecode* frame) {
  ComPtr<IWICMetadataQueryReader> metadata;
  if (FAILED(frame->GetMetadataQueryReader(&metadata))) {
    return 1;
  }

  PROPVARIANT value;
  PropVariantInit(&value);
  HRESULT result =
      metadata->GetMetadataByName(L"/app1/ifd/{ushort=274}", &value);
  if (FAILED(result)) {
    result = metadata->GetMetadataByName(L"/ifd/{ushort=274}", &value);
  }
  if (FAILED(result)) {
    PropVariantClear(&value);
    return 1;
  }

  USHORT orientation = 1;
  if (value.vt == VT_UI2) {
    orientation = value.uiVal;
  } else if (value.vt == VT_UI1) {
    orientation = value.bVal;
  } else if (value.vt == VT_UI4 &&
             value.ulVal <= std::numeric_limits<USHORT>::max()) {
    orientation = static_cast<USHORT>(value.ulVal);
  }
  PropVariantClear(&value);
  return orientation;
}

WICBitmapTransformOptions OrientationTransform(USHORT orientation) {
  switch (orientation) {
    case 2:
      return WICBitmapTransformFlipHorizontal;
    case 3:
      return WICBitmapTransformRotate180;
    case 4:
      return WICBitmapTransformFlipVertical;
    case 5:
      return static_cast<WICBitmapTransformOptions>(
          WICBitmapTransformRotate90 | WICBitmapTransformFlipHorizontal);
    case 6:
      return WICBitmapTransformRotate90;
    case 7:
      return static_cast<WICBitmapTransformOptions>(
          WICBitmapTransformRotate270 | WICBitmapTransformFlipHorizontal);
    case 8:
      return WICBitmapTransformRotate270;
    default:
      return WICBitmapTransformRotate0;
  }
}

ComPtr<IWICBitmapSource> ApplyOrientation(
    IWICImagingFactory* factory, const ComPtr<IWICBitmapFrameDecode>& frame) {
  ComPtr<IWICBitmapSource> source;
  CheckHresult(frame.As(&source), "Preparing the selected image");

  const WICBitmapTransformOptions transform =
      OrientationTransform(ReadExifOrientation(frame.Get()));
  if (transform == WICBitmapTransformRotate0) {
    return source;
  }

  ComPtr<IWICBitmapFlipRotator> rotator;
  CheckHresult(factory->CreateBitmapFlipRotator(&rotator),
               "Creating an image orientation transform");
  CheckHresult(rotator->Initialize(source.Get(), transform),
               "Applying the image orientation");

  ComPtr<IWICBitmapSource> oriented;
  CheckHresult(rotator.As(&oriented), "Preparing the oriented image");
  return oriented;
}

ComPtr<IWICBitmapSource> ResizeImage(IWICImagingFactory* factory,
                                     IWICBitmapSource* source, UINT width,
                                     UINT height) {
  UINT source_width = 0;
  UINT source_height = 0;
  CheckHresult(source->GetSize(&source_width, &source_height),
               "Reading image dimensions");
  if (source_width == width && source_height == height) {
    ComPtr<IWICBitmapSource> unchanged;
    CheckHresult(source->QueryInterface(IID_PPV_ARGS(&unchanged)),
                 "Preparing the selected image");
    return unchanged;
  }

  ComPtr<IWICBitmapScaler> scaler;
  CheckHresult(factory->CreateBitmapScaler(&scaler),
               "Creating an image scaler");
  CheckHresult(
      scaler->Initialize(source, width, height, WICBitmapInterpolationModeFant),
      "Scaling the selected image");

  ComPtr<IWICBitmapSource> resized;
  CheckHresult(scaler.As(&resized), "Preparing the scaled image");
  return resized;
}

ComPtr<IWICBitmap> CompositeOntoWhite(IWICImagingFactory* factory,
                                      IWICBitmapSource* source, UINT width,
                                      UINT height) {
  ComPtr<IWICFormatConverter> converter;
  CheckHresult(factory->CreateFormatConverter(&converter),
               "Creating an image color converter");
  CheckHresult(converter->Initialize(source, GUID_WICPixelFormat32bppBGRA,
                                     WICBitmapDitherTypeNone, nullptr, 0.0,
                                     WICBitmapPaletteTypeCustom),
               "Converting the selected image colors");

  const uint64_t bgra_stride_64 = static_cast<uint64_t>(width) * 4;
  const uint64_t bgra_size_64 = bgra_stride_64 * height;
  const uint64_t bgr_stride_64 = static_cast<uint64_t>(width) * 3;
  const uint64_t bgr_size_64 = bgr_stride_64 * height;
  if (bgra_size_64 > std::numeric_limits<UINT>::max() ||
      bgr_size_64 > std::numeric_limits<UINT>::max()) {
    throw PickerException("image_processing_failed",
                          "The selected image is too large to process.");
  }

  const UINT bgra_stride = static_cast<UINT>(bgra_stride_64);
  const UINT bgra_size = static_cast<UINT>(bgra_size_64);
  const UINT bgr_stride = static_cast<UINT>(bgr_stride_64);
  const UINT bgr_size = static_cast<UINT>(bgr_size_64);
  std::vector<BYTE> bgra(bgra_size);
  std::vector<BYTE> bgr(bgr_size);
  CheckHresult(
      converter->CopyPixels(nullptr, bgra_stride, bgra_size, bgra.data()),
      "Reading the converted image pixels");

  const size_t pixel_count = static_cast<size_t>(width) * height;
  for (size_t pixel = 0; pixel < pixel_count; ++pixel) {
    const size_t source_offset = pixel * 4;
    const size_t target_offset = pixel * 3;
    const uint32_t alpha = bgra[source_offset + 3];
    for (size_t channel = 0; channel < 3; ++channel) {
      const uint32_t color = bgra[source_offset + channel];
      const uint32_t composited = color * alpha + 255U * (255U - alpha);
      bgr[target_offset + channel] =
          static_cast<BYTE>((composited + 127U) / 255U);
    }
  }

  ComPtr<IWICBitmap> bitmap;
  CheckHresult(factory->CreateBitmapFromMemory(
                   width, height, GUID_WICPixelFormat24bppBGR, bgr_stride,
                   bgr_size, bgr.data(), &bitmap),
               "Creating the compressed image source");
  return bitmap;
}

std::vector<uint8_t> EncodeJpeg(IWICImagingFactory* factory,
                                IWICBitmapSource* source, UINT width,
                                UINT height, int quality_percent) {
  const ComPtr<IWICBitmapSource> resized =
      ResizeImage(factory, source, width, height);
  const ComPtr<IWICBitmap> composited =
      CompositeOntoWhite(factory, resized.Get(), width, height);

  ComPtr<IStream> stream;
  CheckHresult(CreateStreamOnHGlobal(nullptr, TRUE, &stream),
               "Creating an image output stream");

  ComPtr<IWICBitmapEncoder> encoder;
  CheckHresult(
      factory->CreateEncoder(GUID_ContainerFormatJpeg, nullptr, &encoder),
      "Creating a JPEG encoder");
  CheckHresult(encoder->Initialize(stream.Get(), WICBitmapEncoderNoCache),
               "Initializing the JPEG encoder");

  ComPtr<IWICBitmapFrameEncode> frame;
  ComPtr<IPropertyBag2> properties;
  CheckHresult(encoder->CreateNewFrame(&frame, &properties),
               "Creating a JPEG frame");

  PROPBAG2 quality_property{};
  quality_property.pstrName = const_cast<wchar_t*>(L"ImageQuality");
  VARIANT quality_value;
  VariantInit(&quality_value);
  quality_value.vt = VT_R4;
  quality_value.fltVal = static_cast<float>(quality_percent) / 100.0F;
  CheckHresult(properties->Write(1, &quality_property, &quality_value),
               "Configuring JPEG quality");
  VariantClear(&quality_value);

  CheckHresult(frame->Initialize(properties.Get()),
               "Initializing the JPEG frame");
  CheckHresult(frame->SetSize(width, height), "Setting JPEG dimensions");
  WICPixelFormatGUID pixel_format = GUID_WICPixelFormat24bppBGR;
  CheckHresult(frame->SetPixelFormat(&pixel_format),
               "Setting the JPEG pixel format");
  CheckHresult(frame->WriteSource(composited.Get(), nullptr),
               "Encoding the selected image");
  CheckHresult(frame->Commit(), "Finalizing the JPEG frame");
  CheckHresult(encoder->Commit(), "Finalizing the JPEG image");

  STATSTG statistics{};
  CheckHresult(stream->Stat(&statistics, STATFLAG_NONAME),
               "Reading the encoded image size");
  if (statistics.cbSize.QuadPart == 0 ||
      statistics.cbSize.QuadPart > std::numeric_limits<size_t>::max()) {
    throw PickerException("image_processing_failed",
                          "The encoded image has an invalid size.");
  }
  const size_t byte_count = static_cast<size_t>(statistics.cbSize.QuadPart);

  HGLOBAL memory = nullptr;
  CheckHresult(GetHGlobalFromStream(stream.Get(), &memory),
               "Reading the encoded image");
  const void* bytes = GlobalLock(memory);
  if (bytes == nullptr) {
    throw PickerException("image_processing_failed",
                          "The encoded image could not be read.");
  }
  std::vector<uint8_t> output(byte_count);
  std::memcpy(output.data(), bytes, byte_count);
  GlobalUnlock(memory);
  return output;
}

std::string OutputName(const std::wstring& path) {
  const size_t separator = path.find_last_of(L"\\/");
  const size_t name_start = separator == std::wstring::npos ? 0 : separator + 1;
  const size_t dot = path.find_last_of(L'.');
  const size_t name_end =
      dot == std::wstring::npos || dot < name_start ? path.size() : dot;
  std::wstring stem = path.substr(name_start, name_end - name_start);
  if (stem.empty()) {
    stem = L"image";
  }
  const std::string utf8_stem = Utf8FromUtf16(stem.c_str());
  return (utf8_stem.empty() ? "image" : utf8_stem) + std::string(".jpg");
}

flutter::EncodableMap PrepareImage(const std::wstring& path) {
  ComPtr<IWICImagingFactory> factory;
  CheckHresult(CoCreateInstance(CLSID_WICImagingFactory, nullptr,
                                CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&factory)),
               "Creating the Windows image processor");

  ComPtr<IWICBitmapDecoder> decoder;
  CheckHresult(factory->CreateDecoderFromFilename(
                   path.c_str(), nullptr, GENERIC_READ,
                   WICDecodeMetadataCacheOnLoad, &decoder),
               "Opening the selected image");

  ComPtr<IWICBitmapFrameDecode> frame;
  CheckHresult(decoder->GetFrame(0, &frame), "Reading the selected image");
  const ComPtr<IWICBitmapSource> source =
      ApplyOrientation(factory.Get(), frame);

  UINT source_width = 0;
  UINT source_height = 0;
  CheckHresult(source->GetSize(&source_width, &source_height),
               "Reading image dimensions");
  if (source_width == 0 || source_height == 0) {
    throw PickerException("image_processing_failed",
                          "The selected file is not a readable image.");
  }

  const UINT longest_side = std::max(source_width, source_height);
  const double initial_scale =
      longest_side > kMaxOutputDimension
          ? static_cast<double>(kMaxOutputDimension) / longest_side
          : 1.0;
  UINT width = std::max(
      1U, static_cast<UINT>(std::lround(source_width * initial_scale)));
  UINT height = std::max(
      1U, static_cast<UINT>(std::lround(source_height * initial_scale)));
  int quality = 88;

  while (true) {
    std::vector<uint8_t> bytes =
        EncodeJpeg(factory.Get(), source.Get(), width, height, quality);
    if (bytes.size() <= kMaxImageBytes) {
      return flutter::EncodableMap{
          {flutter::EncodableValue("name"),
           flutter::EncodableValue(OutputName(path))},
          {flutter::EncodableValue("mimeType"),
           flutter::EncodableValue("image/jpeg")},
          {flutter::EncodableValue("bytes"),
           flutter::EncodableValue(std::move(bytes))},
      };
    }

    if (quality > 48) {
      quality -= 8;
      continue;
    }

    width = std::max(1U, static_cast<UINT>(std::lround(width * 0.82)));
    height = std::max(1U, static_cast<UINT>(std::lround(height * 0.82)));
    if (std::max(width, height) < kMinOutputDimension) {
      throw PickerException(
          "image_processing_failed",
          "The selected image cannot be compressed enough to send.");
    }
    quality = 80;
  }
}

}  // namespace

ImageAttachmentPicker::ImageAttachmentPicker(
    flutter::BinaryMessenger* messenger, HWND owner) {
  channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      messenger, kChannelName, &flutter::StandardMethodCodec::GetInstance());
  channel_->SetMethodCallHandler(
      [owner](const flutter::MethodCall<flutter::EncodableValue>& call,
              std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                  result) {
        if (call.method_name() != "pickImages") {
          result->NotImplemented();
          return;
        }

        try {
          const int max_count = ReadMaxCount(call.arguments());
          const std::vector<std::wstring> paths =
              PickImagePaths(owner, max_count);
          flutter::EncodableList images;
          images.reserve(paths.size());
          for (const std::wstring& path : paths) {
            images.emplace_back(PrepareImage(path));
          }
          result->Success(flutter::EncodableValue(std::move(images)));
        } catch (const PickerException& error) {
          result->Error(error.code(), error.what());
        } catch (const std::exception& error) {
          result->Error("image_processing_failed", error.what());
        }
      });
}

ImageAttachmentPicker::~ImageAttachmentPicker() = default;

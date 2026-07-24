#include "window_controls.h"

#include <dwmapi.h>
#include <flutter/standard_method_codec.h>

#include "title_bar_overlay.h"

namespace {

#ifndef DWMWA_CAPTION_BUTTON_BOUNDS
#define DWMWA_CAPTION_BUTTON_BOUNDS 5
#endif

constexpr char kChannelName[] = "dev.phi.phi_client/window_controls";
constexpr UINT kDefaultDpi = 96;

bool IsMaximized(HWND window) {
  return IsZoomed(window) != FALSE;
}

double CaptionButtonWidth(HWND window) {
  const double overlay_inset = TitleBarOverlayRightInset(window);
  if (overlay_inset > 0) {
    return overlay_inset;
  }

  const UINT window_dpi = GetDpiForWindow(window);
  const UINT dpi = window_dpi == 0 ? kDefaultDpi : window_dpi;

  RECT bounds{};
  if (SUCCEEDED(DwmGetWindowAttribute(window, DWMWA_CAPTION_BUTTON_BOUNDS,
                                      &bounds, sizeof(bounds))) &&
      bounds.right > bounds.left) {
    return static_cast<double>(bounds.right - bounds.left) * kDefaultDpi / dpi;
  }

  const int fallback_width = GetSystemMetricsForDpi(SM_CXSIZE, dpi) * 3;
  return static_cast<double>(fallback_width) * kDefaultDpi / dpi;
}

}  // namespace

WindowControls::WindowControls(flutter::BinaryMessenger* messenger,
                               HWND window) {
  channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      messenger, kChannelName, &flutter::StandardMethodCodec::GetInstance());
  channel_->SetMethodCallHandler(
      [window](const flutter::MethodCall<flutter::EncodableValue>& call,
               std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>>
                   result) {
        if (!IsWindow(window)) {
          result->Error("window_unavailable",
                        "The native Windows window is unavailable.");
          return;
        }

        const std::string& method = call.method_name();
        if (method == "captionButtonWidth") {
          result->Success(
              flutter::EncodableValue(CaptionButtonWidth(window)));
          return;
        }
        if (method == "isMaximized") {
          result->Success(flutter::EncodableValue(IsMaximized(window)));
          return;
        }
        if (method == "minimize") {
          ShowWindow(window, SW_MINIMIZE);
          result->Success();
          return;
        }
        if (method == "toggleMaximize") {
          ShowWindow(window, IsMaximized(window) ? SW_RESTORE : SW_MAXIMIZE);
          result->Success(flutter::EncodableValue(IsMaximized(window)));
          return;
        }
        if (method == "startDragging") {
          POINT cursor{};
          GetCursorPos(&cursor);
          ReleaseCapture();
          SendMessage(window, WM_NCLBUTTONDOWN, HTCAPTION,
                      MAKELPARAM(cursor.x, cursor.y));
          result->Success();
          return;
        }
        if (method == "close") {
          result->Success();
          PostMessage(window, WM_CLOSE, 0, 0);
          return;
        }

        result->NotImplemented();
      });
}

WindowControls::~WindowControls() = default;

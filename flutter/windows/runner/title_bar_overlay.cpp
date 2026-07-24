#include "title_bar_overlay.h"

#include <winrt/Microsoft.UI.Interop.h>
#include <winrt/Microsoft.UI.Windowing.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Graphics.h>
#include <winrt/Windows.UI.h>

#include <algorithm>
#include <array>

namespace {

constexpr UINT kDefaultDpi = 96;

winrt::Microsoft::UI::Windowing::AppWindowTitleBar GetTitleBar(HWND window) {
  const auto window_id =
      winrt::Microsoft::UI::GetWindowIdFromWindow(window);
  const auto app_window =
      winrt::Microsoft::UI::Windowing::AppWindow::GetFromWindowId(window_id);
  return app_window.TitleBar();
}

}  // namespace

bool EnableTitleBarOverlay(HWND window) noexcept {
  try {
    using winrt::Microsoft::UI::Windowing::AppWindowTitleBar;
    using winrt::Microsoft::UI::Windowing::IconShowOptions;

    if (!AppWindowTitleBar::IsCustomizationSupported()) {
      return false;
    }

    const auto title_bar = GetTitleBar(window);
    title_bar.ExtendsContentIntoTitleBar(true);
    title_bar.IconShowOptions(IconShowOptions::HideIconAndSystemMenu);
    const auto transparent =
        winrt::box_value(winrt::Windows::UI::Colors::Transparent())
            .as<winrt::Windows::Foundation::IReference<
                winrt::Windows::UI::Color>>();
    title_bar.ButtonBackgroundColor(transparent);
    title_bar.ButtonInactiveBackgroundColor(transparent);
    UpdateTitleBarOverlayLayout(window);
    return true;
  } catch (...) {
    return false;
  }
}

void UpdateTitleBarOverlayLayout(HWND window) noexcept {
  try {
    const auto title_bar = GetTitleBar(window);
    if (!title_bar.ExtendsContentIntoTitleBar()) {
      return;
    }

    RECT client_rect{};
    if (!GetClientRect(window, &client_rect)) {
      return;
    }

    const int left_inset = title_bar.LeftInset();
    const int right_inset = title_bar.RightInset();
    const int client_width = client_rect.right - client_rect.left;
    const int client_height = client_rect.bottom - client_rect.top;
    const int drag_width =
        std::max(0, client_width - left_inset - right_inset);
    const int drag_height = std::clamp(title_bar.Height(), 0, client_height);
    if (drag_width == 0 || drag_height == 0) {
      title_bar.SetDragRectangles({});
      return;
    }

    const std::array<winrt::Windows::Graphics::RectInt32, 1>
        drag_rectangles{winrt::Windows::Graphics::RectInt32{
            left_inset, 0, drag_width, drag_height}};
    title_bar.SetDragRectangles(drag_rectangles);
  } catch (...) {
    // The overlay is visual enhancement. Keep the window usable if Windows
    // removes customization support while the process is running.
  }
}

double TitleBarOverlayRightInset(HWND window) noexcept {
  try {
    const auto title_bar = GetTitleBar(window);
    if (!title_bar.ExtendsContentIntoTitleBar()) {
      return 0;
    }

    const UINT window_dpi = GetDpiForWindow(window);
    const UINT dpi = window_dpi == 0 ? kDefaultDpi : window_dpi;
    return static_cast<double>(title_bar.RightInset()) * kDefaultDpi / dpi;
  } catch (...) {
    return 0;
  }
}

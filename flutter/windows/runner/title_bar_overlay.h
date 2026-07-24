#ifndef RUNNER_TITLE_BAR_OVERLAY_H_
#define RUNNER_TITLE_BAR_OVERLAY_H_

#include <windows.h>

// Enables the Windows App SDK title-bar overlay for |window|. The system keeps
// ownership of the caption buttons while Flutter can render behind them.
bool EnableTitleBarOverlay(HWND window) noexcept;

// Updates the system drag region after the top-level client area changes.
void UpdateTitleBarOverlayLayout(HWND window) noexcept;

// Returns the system-reserved upper-right inset in Flutter logical pixels.
// Returns zero when the title-bar overlay is unavailable.
double TitleBarOverlayRightInset(HWND window) noexcept;

#endif  // RUNNER_TITLE_BAR_OVERLAY_H_

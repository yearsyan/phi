#ifndef RUNNER_WINDOW_CONTROLS_H_
#define RUNNER_WINDOW_CONTROLS_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>
#include <windows.h>

#include <memory>

// Owns the Windows implementation of the custom title-bar platform channel.
class WindowControls {
 public:
  WindowControls(flutter::BinaryMessenger* messenger, HWND window);
  ~WindowControls();

  WindowControls(const WindowControls&) = delete;
  WindowControls& operator=(const WindowControls&) = delete;

 private:
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
};

#endif  // RUNNER_WINDOW_CONTROLS_H_

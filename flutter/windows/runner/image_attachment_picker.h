#ifndef RUNNER_IMAGE_ATTACHMENT_PICKER_H_
#define RUNNER_IMAGE_ATTACHMENT_PICKER_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>
#include <windows.h>

#include <memory>

// Owns the Windows implementation of the image attachment platform channel.
class ImageAttachmentPicker {
 public:
  ImageAttachmentPicker(flutter::BinaryMessenger* messenger, HWND owner);
  ~ImageAttachmentPicker();

  ImageAttachmentPicker(const ImageAttachmentPicker&) = delete;
  ImageAttachmentPicker& operator=(const ImageAttachmentPicker&) = delete;

 private:
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
};

#endif  // RUNNER_IMAGE_ATTACHMENT_PICKER_H_

import 'dart:convert';

import 'package:flutter/services.dart';

class PickedImageAttachment {
  const PickedImageAttachment({
    required this.name,
    required this.mimeType,
    required this.bytes,
  });

  final String name;
  final String mimeType;
  final Uint8List bytes;

  String get dataUrl => 'data:$mimeType;base64,${base64Encode(bytes)}';
}

class ImageAttachmentPicker {
  static const _channel = MethodChannel(
    'dev.phi.phi_client/image_attachment_picker',
  );

  static Future<List<PickedImageAttachment>> pickImages({
    required int maxCount,
  }) async {
    final values = await _channel.invokeListMethod<Object?>('pickImages', {
      'maxCount': maxCount,
    });
    if (values == null) return const [];

    return [
      for (final value in values)
        if (value is Map)
          PickedImageAttachment(
            name: value['name'] as String? ?? 'image.jpg',
            mimeType: value['mimeType'] as String? ?? 'image/jpeg',
            bytes: value['bytes'] as Uint8List,
          ),
    ];
  }
}

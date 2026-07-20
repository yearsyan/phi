import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;
import 'package:http/io_client.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

import 'daemon_transport.dart';

/// Direct TCP transport: plain HTTP/HTTPS for REST and WS/WSS for streaming.
///
/// This is the default transport. Future transports (HTTP over SSH,
/// HTTP over Tailscale) implement [DaemonTransport] against the same
/// interface and can be selected at the connection-settings level.
class DirectDaemonTransport implements DaemonTransport {
  DirectDaemonTransport({
    required this.baseUri,
    required this.authKey,
    this.allowUntrustedCerts = false,
  }) {
    final httpClient = HttpClient()
      ..connectionTimeout = const Duration(seconds: 15)
      ..badCertificateCallback = (cert, host, port) => allowUntrustedCerts;
    _http = IOClient(httpClient);
  }

  /// Base URI of the daemon, e.g. `http://127.0.0.1:8787` or
  /// `https://daemon.example.com`. Must not end with `/`.
  final Uri baseUri;
  final String authKey;
  final bool allowUntrustedCerts;

  late final http.Client _http;

  @override
  String get displayName => baseUri.toString();

  Uri _resolve(String path, Map<String, String>? query) {
    final base = baseUri.toString().endsWith('/')
        ? baseUri.toString().substring(0, baseUri.toString().length - 1)
        : baseUri.toString();
    final uri = Uri.parse('$base$path');
    if (query != null && query.isNotEmpty) {
      return uri.replace(queryParameters: query);
    }
    return uri;
  }

  Uri _resolveWs(String path, Map<String, String>? query) {
    final httpUri = _resolve(path, query);
    final scheme = switch (httpUri.scheme) {
      'https' || 'wss' => 'wss',
      _ => 'ws',
    };
    return httpUri.replace(scheme: scheme);
  }

  Map<String, String> get _headers => {
    'authorization': 'Bearer $authKey',
    'content-type': 'application/json',
  };

  @override
  Future<DaemonHttpResponse> request(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  }) async {
    final uri = _resolve(path, query);
    try {
      final request = http.Request(method.toUpperCase(), uri)
        ..headers.addAll(_headers);
      if (body != null) {
        request.body = jsonEncode(body);
      }
      final streamed = await _http
          .send(request)
          .timeout(const Duration(seconds: 30));
      final response = await http.Response.fromStream(streamed);
      return DaemonHttpResponse(
        response.statusCode,
        response.body,
        response.headers.map((k, v) => MapEntry(k.toLowerCase(), v)),
      );
    } on TimeoutException catch (e) {
      throw DaemonTransportException(
        'request timed out: $method $path',
        cause: e,
      );
    } on SocketException catch (e) {
      throw DaemonTransportException(
        'cannot reach daemon at ${baseUri.host}:${baseUri.port} (${e.message})',
        cause: e,
      );
    } on http.ClientException catch (e) {
      throw DaemonTransportException(
        'connection failed: ${e.message}',
        cause: e,
      );
    }
  }

  @override
  Future<DaemonSocket> connect(
    String path, {
    Map<String, String>? query,
    List<String> protocols = const [],
    Duration? timeout,
  }) async {
    final uri = _resolveWs(path, query);
    try {
      // Note: WebSocketChannel.connect uses the default dart:io WebSocket
      // implementation. WSS with untrusted certificates is not supported by
      // this transport; use a trusted cert or plain WS on a trusted link.
      final channel = WebSocketChannel.connect(uri, protocols: protocols);
      await channel.ready.timeout(timeout ?? const Duration(seconds: 15));
      return _WebSocketDaemonSocket(channel);
    } on TimeoutException catch (e) {
      throw DaemonTransportException(
        'websocket connect timed out: $path',
        cause: e,
      );
    } on WebSocketChannelException catch (e) {
      throw DaemonTransportException(
        'websocket connect failed: ${e.message}',
        cause: e,
      );
    } on SocketException catch (e) {
      throw DaemonTransportException(
        'websocket connect failed: ${e.message}',
        cause: e,
      );
    }
  }

  @override
  void dispose() {
    _http.close();
  }
}

class _WebSocketDaemonSocket implements DaemonSocket {
  _WebSocketDaemonSocket(this._channel);

  final WebSocketChannel _channel;

  @override
  Stream<String> get messages => _channel.stream.map((event) {
    if (event is String) return event;
    if (event is List<int>) return utf8.decode(event);
    return event.toString();
  });

  @override
  void send(String message) => _channel.sink.add(message);

  @override
  Future<void> close() => _channel.sink.close();
}

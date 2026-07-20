/// Pluggable transport abstraction for talking to a phi daemon.
///
/// The daemon speaks HTTP(S) for REST endpoints and WebSocket for session
/// streams. All daemon access goes through [DaemonTransport] so that the
/// underlying channel can be swapped: today a direct TCP connection
/// (`DirectDaemonTransport`), in the future HTTP-over-SSH or
/// HTTP-over-Tailscale transports implementing this same interface.
library;

/// Result of an HTTP-style request against the daemon.
class DaemonHttpResponse {
  const DaemonHttpResponse(this.statusCode, this.body, this.headers);

  final int statusCode;
  final String body;
  final Map<String, String> headers;

  bool get isSuccess => statusCode >= 200 && statusCode < 300;
}

/// A streaming, message-oriented connection (WebSocket semantics).
abstract class DaemonSocket {
  /// Incoming text frames, already decoded to UTF-8 strings.
  Stream<String> get messages;

  /// Send a single text frame.
  void send(String message);

  /// Close the connection.
  Future<void> close();
}

/// Errors raised by transports.
class DaemonTransportException implements Exception {
  DaemonTransportException(this.message, {this.cause});

  final String message;
  final Object? cause;

  @override
  String toString() => 'DaemonTransportException: $message';
}

/// Pluggable daemon transport.
///
/// Implementations must provide request/response HTTP semantics and a
/// message-oriented streaming channel with WebSocket subprotocol support
/// (the daemon authenticates sockets via a `phi.auth.<token>` subprotocol).
abstract class DaemonTransport {
  /// Human-readable description of where this transport points (for UI).
  String get displayName;

  /// Perform an HTTP request. [path] is the absolute path starting with `/`,
  /// e.g. `/v1/sessions`. [body], when non-null, is a JSON-encodable value.
  Future<DaemonHttpResponse> request(
    String method,
    String path, {
    Map<String, String>? query,
    Object? body,
  });

  /// Open a streaming channel to [path] (e.g. `/v1/ws/attach/<id>`).
  ///
  /// [protocols] are the WebSocket subprotocols to offer during the
  /// handshake; transports that tunnel over a different substrate must map
  /// these onto their equivalent negotiation mechanism.
  Future<DaemonSocket> connect(
    String path, {
    Map<String, String>? query,
    List<String> protocols = const [],
    Duration? timeout,
  });

  /// Release any resources held by the transport.
  void dispose();
}

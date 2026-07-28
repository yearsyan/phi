import Cocoa
import FlutterMacOS
import LocalAuthentication
import Security

private let promptFreeKeychainChannelName =
  "dev.phi.phi_client/prompt_free_keychain"

private enum PromptFreeKeychainReadResult {
  case value(String?)
  case error(FlutterError)
}

/// Stores daemon keys in the user's encrypted login Keychain without ever
/// allowing Keychain Services to display authentication UI.
///
/// Phi used flutter_secure_storage's default legacy service before the macOS
/// app gained a stable Developer ID signature. Those entries can carry an ACL
/// for an old ad-hoc build. We migrate an old entry only when it is already
/// accessible without interaction; otherwise it is left untouched and
/// reported as missing so app launch never raises a password dialog.
private struct PromptFreeKeychain {
  private static let signedService = "dev.phi.phiClient.daemon-auth.v2"
  private static let localService = "dev.phi.phiClient.daemon-auth.local.v2"
  private static let legacyService = "flutter_secure_storage_service"
  private static let service = currentService()

  private enum Lookup {
    case value(String)
    case missing
    case inaccessible
    case failed(OSStatus)
  }

  func read(key: String) -> PromptFreeKeychainReadResult {
    switch lookup(service: Self.service, key: key) {
    case .value(let value):
      return .value(value)
    case .missing:
      return readLegacyAndMigrate(key: key)
    case .inaccessible:
      return .value(nil)
    case .failed(let status):
      return .error(error(operation: "read", status: status))
    }
  }

  func write(key: String, value: String) -> FlutterError? {
    let status = upsert(service: Self.service, key: key, value: value)
    guard status == errSecSuccess else {
      return error(operation: "write", status: status)
    }
    return nil
  }

  func delete(key: String) -> FlutterError? {
    let status = SecItemDelete(
      query(service: Self.service, key: key) as CFDictionary
    )
    guard status == errSecSuccess || status == errSecItemNotFound else {
      return error(operation: "delete", status: status)
    }
    return nil
  }

  private func readLegacyAndMigrate(
    key: String
  ) -> PromptFreeKeychainReadResult {
    switch lookup(service: Self.legacyService, key: key) {
    case .value(let value):
      // Migration is best effort. Returning the already-authorized value keeps
      // this launch usable even if the destination Keychain is locked.
      if upsert(service: Self.service, key: key, value: value) == errSecSuccess {
        _ = SecItemDelete(
          query(service: Self.legacyService, key: key) as CFDictionary
        )
      }
      return .value(value)
    case .missing, .inaccessible:
      return .value(nil)
    case .failed(let status):
      return .error(error(operation: "read legacy", status: status))
    }
  }

  private func lookup(service: String, key: String) -> Lookup {
    var item: CFTypeRef?
    var attributes = query(service: service, key: key)
    attributes[kSecReturnData] = true
    attributes[kSecMatchLimit] = kSecMatchLimitOne

    let status = SecItemCopyMatching(attributes as CFDictionary, &item)
    switch status {
    case errSecSuccess:
      guard
        let data = item as? Data,
        let value = String(data: data, encoding: .utf8)
      else {
        return .failed(errSecDecode)
      }
      return .value(value)
    case errSecItemNotFound:
      return .missing
    case errSecInteractionNotAllowed, errSecAuthFailed, errSecUserCanceled:
      return .inaccessible
    default:
      return .failed(status)
    }
  }

  private func upsert(service: String, key: String, value: String) -> OSStatus {
    guard let data = value.data(using: .utf8) else {
      return errSecParam
    }

    let attributes = query(service: service, key: key)
    let update = [kSecValueData: data] as CFDictionary
    let updateStatus = SecItemUpdate(attributes as CFDictionary, update)
    guard updateStatus == errSecItemNotFound else {
      return updateStatus
    }

    var addition = attributes
    addition[kSecValueData] = data
    return SecItemAdd(addition as CFDictionary, nil)
  }

  private func query(service: String, key: String) -> [CFString: Any] {
    let context = LAContext()
    context.interactionNotAllowed = true
    return [
      kSecClass: kSecClassGenericPassword,
      kSecAttrService: service,
      kSecAttrAccount: key,
      kSecUseAuthenticationContext: context,
    ]
  }

  private static func currentService() -> String {
    var code: SecCode?
    guard
      SecCodeCopySelf(SecCSFlags(rawValue: 0), &code) == errSecSuccess,
      let code
    else {
      return localService
    }

    var staticCode: SecStaticCode?
    guard
      SecCodeCopyStaticCode(
        code,
        SecCSFlags(rawValue: 0),
        &staticCode
      ) == errSecSuccess,
      let staticCode
    else {
      return localService
    }

    var information: CFDictionary?
    guard
      SecCodeCopySigningInformation(
        staticCode,
        SecCSFlags(rawValue: kSecCSSigningInformation),
        &information
      ) == errSecSuccess,
      let values = information as? [String: Any],
      let teamIdentifier =
        values[kSecCodeInfoTeamIdentifier as String] as? String,
      !teamIdentifier.isEmpty
    else {
      return localService
    }
    return signedService
  }

  private func error(operation: String, status: OSStatus) -> FlutterError {
    FlutterError(
      code: "keychain_error",
      message: "Could not \(operation) daemon credential (OSStatus \(status)).",
      details: nil
    )
  }
}

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    // Keep the native title bar (and its traffic-light controls), but let the
    // Flutter surface paint behind it so the app chrome reaches the window's
    // top edge.
    self.styleMask.insert(.fullSizeContentView)
    self.titleVisibility = .hidden
    self.titlebarAppearsTransparent = true
    if #available(macOS 11.0, *) {
      self.titlebarSeparatorStyle = .none
    }

    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)
    let promptFreeKeychain = PromptFreeKeychain()
    let promptFreeKeychainChannel = FlutterMethodChannel(
      name: promptFreeKeychainChannelName,
      binaryMessenger: flutterViewController.engine.binaryMessenger
    )
    promptFreeKeychainChannel.setMethodCallHandler { call, result in
      guard
        let arguments = call.arguments as? [String: Any],
        let key = arguments["key"] as? String,
        !key.isEmpty
      else {
        result(
          FlutterError(
            code: "invalid_keychain_arguments",
            message: "A non-empty credential key is required.",
            details: nil
          )
        )
        return
      }

      switch call.method {
      case "read":
        switch promptFreeKeychain.read(key: key) {
        case .value(let value):
          result(value)
        case .error(let error):
          result(error)
        }
      case "write":
        guard let value = arguments["value"] as? String else {
          result(
            FlutterError(
              code: "invalid_keychain_arguments",
              message: "A credential value is required.",
              details: nil
            )
          )
          return
        }
        if let error = promptFreeKeychain.write(key: key, value: value) {
          result(error)
        } else {
          result(nil)
        }
      case "delete":
        if let error = promptFreeKeychain.delete(key: key) {
          result(error)
        } else {
          result(nil)
        }
      default:
        result(FlutterMethodNotImplemented)
      }
    }

    super.awakeFromNib()
  }
}

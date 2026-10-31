// Prints the Sparkle public key for $SPARKLE_PRIVATE_KEY, the base64 Ed25519
// seed that Sparkle's `generate_keys -x` exports. scripts/release.sh compares
// it with SUPublicEDKey in Info.plist.
import CryptoKit
import Foundation

let encoded = ProcessInfo.processInfo.environment["SPARKLE_PRIVATE_KEY"] ?? ""
guard
    let seed = Data(base64Encoded: encoded.trimmingCharacters(in: .whitespacesAndNewlines)),
    let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: seed)
else {
    FileHandle.standardError.write(Data("SPARKLE_PRIVATE_KEY isn't a base64 Ed25519 seed.\n".utf8))
    exit(1)
}
print(key.publicKey.rawRepresentation.base64EncodedString())

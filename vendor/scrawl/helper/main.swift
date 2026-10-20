// scrawl-helper: the parts of Annotate (Scrawl in Bespoke) that need Swift-only Apple frameworks.
//
//   scrawl-helper transcribe FILE                       SpeechAnalyzer transcript with word timings
//   scrawl-helper speech-model [--install]              the speech model's status; install it (JSON lines)
//   scrawl-helper frames MOVIE --out-dir DIR (--every MS | --at MS,MS…) [--max-edge PX]
//   scrawl-helper permissions                           what Bespoke has been granted, as JSON
//   scrawl-helper permission-panel --permission P [--from X,Y,W,H]   drag-to-allow card (PermissionPanel.swift)
//
// Every timestamp in the output is integer milliseconds.

import AVFoundation
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

func emit(_ object: Any) {
  guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]) else { return }
  FileHandle.standardOutput.write(data + Data("\n".utf8))
}

func fail(_ message: String) -> Never {
  FileHandle.standardError.write(Data("scrawl-helper: \(message)\n".utf8))
  exit(1)
}

struct HelperError: LocalizedError {
  let errorDescription: String?
  init(_ message: String) { errorDescription = message }
}

func option(_ name: String, in arguments: [String]) -> String? {
  guard let index = arguments.firstIndex(of: name), index + 1 < arguments.count else { return nil }
  return arguments[index + 1]
}

/// Runs async work from a synchronous entry point, keeping the main queue free for AVFoundation.
func runAsync(_ body: @escaping () async throws -> Void) -> Never {
  Task {
    do {
      try await body()
      exit(0)
    } catch {
      fail(error.localizedDescription)
    }
  }
  dispatchMain()
}

enum Permissions {
  static func statuses() -> [String: String] {
    func av(_ type: AVMediaType) -> String {
      switch AVCaptureDevice.authorizationStatus(for: type) {
      case .authorized: return "granted"
      case .denied, .restricted: return "denied"
      default: return "not-determined"
      }
    }
    return [
      "screen-recording": CGPreflightScreenCaptureAccess() ? "granted" : "not-determined",
      "microphone": av(.audio),
      "accessibility": AXIsProcessTrusted() ? "granted" : "not-determined",
    ]
  }
}

let arguments = Array(CommandLine.arguments.dropFirst())
switch arguments.first {
case "transcribe":
  guard arguments.count >= 2 else { fail("transcribe needs a file") }
  runAsync { emit(try await Transcriber.transcribe(URL(filePath: arguments[1]))) }
case "speech-model":
  let install = arguments.contains("--install")
  runAsync { await SpeechModel.run(install: install) }
case "frames":
  guard arguments.count >= 2, let dir = option("--out-dir", in: arguments) else {
    fail("frames needs MOVIE --out-dir DIR (--every MS | --at MS,MS…)")
  }
  let maxEdge = option("--max-edge", in: arguments).flatMap(Int.init) ?? 0
  let every = option("--every", in: arguments).flatMap(Int.init)
  let at = option("--at", in: arguments)?.split(separator: ",").compactMap { Int($0) }
  runAsync {
    emit(try await Media.frames(movie: URL(filePath: arguments[1]), outDir: URL(filePath: dir), everyMs: every, atMs: at, maxEdge: maxEdge))
  }
case "permission-panel":
  PermissionPanel.run(arguments: arguments)
case "permissions":
  emit(Permissions.statuses())
default:
  fail("usage: scrawl-helper transcribe|speech-model|frames|permissions|permission-panel …")
}

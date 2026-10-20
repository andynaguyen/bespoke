@preconcurrency import AVFoundation
import CoreGraphics
import Speech

/// The on-device model that transcribes: SpeechTranscriber where this Mac supports it, else
/// DictationTranscriber (Macs, and virtual machines, without SpeechTranscriber), for the user's
/// locale, the nearest supported one in the same language, or en-US.
struct SpeechChoice {
  let dictation: Bool
  let locale: Locale

  var name: String { dictation ? "DictationTranscriber" : "SpeechTranscriber" }

  func module() -> any SpeechModule {
    dictation
      ? DictationTranscriber(locale: locale, contentHints: [], transcriptionOptions: [.punctuation], reportingOptions: [], attributeOptions: [.audioTimeRange])
      : SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [], attributeOptions: [.audioTimeRange])
  }
}

/// Apple's speech assets are installed per app: `AssetInventory` reserves the locale for this
/// bundle id (`dev.kitlangton.scrawl.helper`) and downloads the model if the Mac doesn't have it.
/// A fresh Mac has none, so Scrawl installs it in the background at launch
/// (`scrawl-helper speech-model --install`) rather than in the middle of a capture.
enum SpeechModel {
  static func choose() async -> SpeechChoice? {
    if SpeechTranscriber.isAvailable,
       let locale = pick(await SpeechTranscriber.supportedLocales, equivalent: await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current))
    {
      return SpeechChoice(dictation: false, locale: locale)
    }
    if let locale = pick(await DictationTranscriber.supportedLocales, equivalent: await DictationTranscriber.supportedLocale(equivalentTo: Locale.current)) {
      return SpeechChoice(dictation: true, locale: locale)
    }
    return nil
  }

  /// `equivalent` only counts when it's really in `supported` (macOS 26 names en_US even where
  /// SpeechTranscriber supports nothing).
  private static func pick(_ supported: [Locale], equivalent: Locale?) -> Locale? {
    let ids = supported.map { $0.identifier(.bcp47) }
    if let equivalent, ids.contains(equivalent.identifier(.bcp47)) { return equivalent }
    let language = Locale.current.language.languageCode
    let region = Locale.current.region
    let sameLanguage = supported.filter { $0.language.languageCode == language }
    if let match = sameLanguage.first(where: { $0.region == region }) ?? sameLanguage.first { return match }
    return supported.first { $0.identifier(.bcp47) == "en-US" }
  }

  static func status(_ choice: SpeechChoice) async -> String {
    switch await AssetInventory.status(forModules: [choice.module()]) {
    case .installed: return "installed"
    case .downloading: return "downloading"
    case .supported: return "supported"
    case .unsupported: return "unsupported"
    @unknown default: return "unsupported"
    }
  }

  static func describe(_ choice: SpeechChoice?, status: String) -> [String: Any] {
    guard let choice else { return ["status": "unsupported"] }
    return ["engine": choice.name, "locale": choice.locale.identifier, "status": status]
  }

  /// `speech-model [--install]`: one status line; with `--install`, installs the model if it
  /// isn't, with progress lines about once a second and a last line that's `installed` or
  /// `failed` (with `error`).
  static func run(install: Bool) async {
    let choice = await choose()
    guard let choice else {
      emit(describe(nil, status: "unsupported"))
      return
    }
    let status = await status(choice)
    emit(describe(choice, status: status))
    guard install, status != "installed", status != "unsupported" else { return }
    do {
      try await self.install(choice) { fraction in
        var line = describe(choice, status: "downloading")
        line["progress"] = fraction
        emit(line)
      }
      emit(describe(choice, status: "installed"))
    } catch {
      var line = describe(choice, status: "failed")
      line["error"] = error.localizedDescription
      emit(line)
    }
  }

  static func install(_ choice: SpeechChoice, progress: @escaping @Sendable (Double) -> Void) async throws {
    guard let request = try await AssetInventory.assetInstallationRequest(supporting: [choice.module()]) else { return }
    let reporter = Task {
      while !Task.isCancelled {
        progress(request.progress.fractionCompleted)
        try? await Task.sleep(nanoseconds: 1_000_000_000)
      }
    }
    defer { reporter.cancel() }
    try await request.downloadAndInstall()
  }
}

/// On-device transcription with SpeechAnalyzer: finalized results only, each word with its own
/// audio time range. Without a usable model the output says why in `missing` (`downloading`,
/// `unsupported` or `failed`, with `error`) and has no text, rather than failing.
enum Transcriber {
  static func transcribe(_ url: URL) async throws -> [String: Any] {
    guard let choice = await SpeechModel.choose() else {
      return missing(nil, "unsupported")
    }
    switch await SpeechModel.status(choice) {
    case "installed": break
    case "downloading": return missing(choice, "downloading")
    case "unsupported": return missing(choice, "unsupported")
    default:
      // Supported but not installed for Scrawl: usually on disk already and only needs
      // reserving, which is quick. A real download is left to the background install.
      do {
        let done = try await withThrowingTaskGroup(of: Bool.self) { group in
          group.addTask { try await SpeechModel.install(choice) { _ in }; return true }
          group.addTask { try await Task.sleep(nanoseconds: 15_000_000_000); return false }
          let first = try await group.next() ?? false
          group.cancelAll()
          return first
        }
        if !done { return missing(choice, "downloading") }
      } catch {
        var result = missing(choice, "failed")
        result["error"] = error.localizedDescription
        return result
      }
    }
    let locale = choice.locale
    if let chunks = try? splitWavAtSilences(url), chunks.files.count > 1 {
      defer { try? FileManager.default.removeItem(at: chunks.dir) }
      let ordered = try await withThrowingTaskGroup(of: (Int, [[String: Any]], [[String: Any]]).self) { group in
        for (idx, item) in chunks.files.enumerated() {
          group.addTask {
            let (segs, wrds) = try await transcribeSingle(item.url, choice: choice, offsetMs: item.offsetMs)
            return (idx, segs, wrds)
          }
        }
        var collected: [(Int, [[String: Any]], [[String: Any]])] = []
        for try await res in group {
          collected.append(res)
        }
        return collected.sorted { $0.0 < $1.0 }
      }
      var segments: [[String: Any]] = []
      var words: [[String: Any]] = []
      for (_, segs, wrds) in ordered {
        segments.append(contentsOf: segs)
        words.append(contentsOf: wrds)
      }
      return [
        "engine": choice.name,
        "locale": locale.identifier,
        "text": segments.compactMap { $0["text"] as? String }.joined(separator: " "),
        "segments": segments,
        "words": words,
      ]
    }
    let (segments, words) = try await transcribeSingle(url, choice: choice, offsetMs: 0)
    return [
      "engine": choice.name,
      "locale": locale.identifier,
      "text": segments.compactMap { $0["text"] as? String }.joined(separator: " "),
      "segments": segments,
      "words": words,
    ]
  }

  private static func missing(_ choice: SpeechChoice?, _ reason: String) -> [String: Any] {
    var result: [String: Any] = ["locale": choice?.locale.identifier ?? Locale.current.identifier, "text": "", "segments": [], "words": [], "missing": reason]
    if let choice { result["engine"] = choice.name }
    return result
  }

  private static func transcribeSingle(_ url: URL, choice: SpeechChoice, offsetMs: Int) async throws -> ([[String: Any]], [[String: Any]]) {
    let file = try AVAudioFile(forReading: url)
    var segments: [[String: Any]] = []
    var words: [[String: Any]] = []
    func collect(_ text: AttributedString, _ range: CMTimeRange) {
      let line = String(text.characters).trimmingCharacters(in: .whitespacesAndNewlines)
      guard !line.isEmpty else { return }
      for run in text.runs {
        guard let time = run.audioTimeRange else { continue }
        let word = String(text[run.range].characters).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !word.isEmpty else { continue }
        words.append(["t0": ms(time.start) + offsetMs, "t1": ms(time.end) + offsetMs, "text": word])
      }
      segments.append(["t0": ms(range.start) + offsetMs, "t1": ms(range.end) + offsetMs, "text": line])
    }
    let module = choice.module()
    let analyzer = try await SpeechAnalyzer(inputAudioFile: file, modules: [module], finishAfterFile: true)
    if let transcriber = module as? SpeechTranscriber {
      for try await result in transcriber.results { collect(result.text, result.range) }
    } else if let transcriber = module as? DictationTranscriber {
      for try await result in transcriber.results where result.isFinal { collect(result.text, result.range) }
    }
    withExtendedLifetime(analyzer) {}
    return (segments, words)
  }

  private struct WavChunks {
    let dir: URL
    let files: [(offsetMs: Int, url: URL)]
  }

  /// Splits a 16 kHz mono 16-bit WAV longer than 22 s at silence midpoints (>= 300 ms) so
  /// long recordings transcribe across concurrent SpeechAnalyzer instances without cutting words.
  private static func splitWavAtSilences(_ url: URL) throws -> WavChunks? {
    let attrs = try FileManager.default.attributesOfItem(atPath: url.path)
    guard let fileSize = attrs[.size] as? Int, fileSize >= 22 * 32000 + 44 else { return nil }
    let data = try Data(contentsOf: url)
    guard data.count >= 44 else { return nil }
    let pcmStart = 44
    let pcmBytes = data.count - pcmStart
    let sampleCount = pcmBytes / 2
    let durationMs = sampleCount / 16
    guard durationMs >= 22000 else { return nil }

    let windowSamples = 800 // 50 ms at 16 kHz
    let thresholdSq = (0.012 * 32768.0) * (0.012 * 32768.0)
    var silences: [(midMs: Int, lenMs: Int)] = []
    data.withUnsafeBytes { rawBuf in
      let base = rawBuf.baseAddress!.advanced(by: pcmStart).assumingMemoryBound(to: Int16.self)
      var silenceStartMs: Int? = nil
      var i = 0
      while i + windowSamples <= sampleCount {
        var sumSq: Double = 0
        for j in 0..<windowSamples {
          let v = Double(Int16(littleEndian: base[i + j]))
          sumSq += v * v
        }
        let meanSq = sumSq / Double(windowSamples)
        let tMs = i / 16
        if meanSq <= thresholdSq {
          if silenceStartMs == nil { silenceStartMs = tMs }
        } else if let s0 = silenceStartMs {
          if tMs - s0 >= 300 {
            silences.append(((s0 + tMs) / 2, tMs - s0))
          }
          silenceStartMs = nil
        }
        i += windowSamples
      }
    }
    guard !silences.isEmpty else { return nil }
    let targetChunks = min(6, max(2, (durationMs + 19999) / 20000))
    var cuts = [0]
    for k in 1..<targetChunks {
      let ideal = (durationMs * k) / targetChunks
      let candidates = silences.filter { abs($0.midMs - ideal) <= 8000 && $0.midMs > cuts.last! + 5000 && $0.midMs < durationMs - 5000 }
      if let best = candidates.min(by: { abs($0.midMs - ideal) < abs($1.midMs - ideal) }) {
        cuts.append(best.midMs)
      }
    }
    cuts.append(durationMs)
    guard cuts.count > 2 else { return nil }

    let dir = FileManager.default.temporaryDirectory.appending(path: "scrawl-wav-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    var files: [(offsetMs: Int, url: URL)] = []
    for idx in 0..<(cuts.count - 1) {
      let c0 = cuts[idx]
      let c1 = cuts[idx + 1]
      let byte0 = pcmStart + c0 * 32
      let byte1 = min(data.count, pcmStart + c1 * 32)
      let sliceLen = byte1 - byte0
      guard sliceLen > 0 else { continue }
      var header = Data()
      func u32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { header.append(contentsOf: $0) } }
      func u16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { header.append(contentsOf: $0) } }
      header.append(contentsOf: Array("RIFF".utf8)); u32(UInt32(36 + sliceLen))
      header.append(contentsOf: Array("WAVE".utf8))
      header.append(contentsOf: Array("fmt ".utf8)); u32(16); u16(1); u16(1); u32(16000); u32(32000); u16(2); u16(16)
      header.append(contentsOf: Array("data".utf8)); u32(UInt32(sliceLen))
      var chunkData = Data(capacity: 44 + sliceLen)
      chunkData.append(header)
      chunkData.append(data[byte0..<byte1])
      let chunkUrl = dir.appending(path: "chunk-\(idx).wav")
      try chunkData.write(to: chunkUrl)
      files.append((c0, chunkUrl))
    }
    return WavChunks(dir: dir, files: files)
  }

  private static func ms(_ time: CMTime) -> Int {
    time.seconds.isFinite ? max(0, Int((time.seconds * 1000).rounded())) : 0
  }
}

enum Media {
  static func frames(movie: URL, outDir: URL, everyMs: Int?, atMs: [Int]?, maxEdge: Int) async throws -> [[String: Any]] {
    let asset = AVURLAsset(url: movie)
    let duration = try await asset.load(.duration).seconds
    let durationMs = Int((duration.isFinite ? duration : 0) * 1000)
    guard durationMs > 0 else { throw HelperError("Movie has no duration") }
    try FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

    let isEvery = (everyMs ?? 0) > 0
    let requested: [Int] = if let everyMs, everyMs > 0 {
      Array(stride(from: 0, to: durationMs, by: everyMs))
    } else {
      (atMs ?? []).map { min(max(0, $0), max(0, durationMs - 1)) }
    }
    guard !requested.isEmpty else { return [] }
    let tolerance = isEvery
      ? CMTime(value: CMTimeValue((everyMs ?? 1) / 2), timescale: 1000)
      : CMTime(value: 80, timescale: 1000)
    let workerCount = min(6, requested.count)
    let chunkSize = (requested.count + workerCount - 1) / workerCount

    let indexed = try await withThrowingTaskGroup(of: [(Int, [String: Any])].self) { group in
      for worker in 0..<workerCount {
        let start = worker * chunkSize
        let end = min(start + chunkSize, requested.count)
        if start >= end { continue }
        let sliceReq = Array(requested[start..<end])
        group.addTask {
          let generator = AVAssetImageGenerator(asset: asset)
          generator.appliesPreferredTrackTransform = true
          if maxEdge > 0 { generator.maximumSize = CGSize(width: maxEdge, height: maxEdge) }
          generator.requestedTimeToleranceBefore = tolerance
          generator.requestedTimeToleranceAfter = tolerance
          let sliceTimes = sliceReq.map { CMTime(value: CMTimeValue($0), timescale: 1000) }
          var local: [(Int, [String: Any])] = []
          var offset = 0
          for await result in generator.images(for: sliceTimes) {
            let curIndex = start + offset
            offset += 1
            guard let image = try? result.image else { continue }
            let reqTime = result.requestedTime.seconds.isFinite
              ? Int((result.requestedTime.seconds * 1000).rounded())
              : sliceReq[min(offset - 1, sliceReq.count - 1)]
            let actual = (try? result.actualTime).map { Int(($0.seconds * 1000).rounded()) } ?? reqTime
            let file = outDir.appending(path: String(format: "frame-%04d.rgba", curIndex))
            try writeRGBA(image, to: file)
            local.append((curIndex, ["t": reqTime, "actual": actual, "width": image.width, "height": image.height, "file": file.path]))
          }
          return local
        }
      }
      var pairs: [(Int, [String: Any])] = []
      for try await batch in group {
        pairs.append(contentsOf: batch)
      }
      return pairs
    }
    return indexed.sorted { $0.0 < $1.0 }.map(\.1)
  }

  private static let srgbSpace = CGColorSpace(name: CGColorSpace.sRGB)!

  private static func writeRGBA(_ image: CGImage, to url: URL) throws {
    let width = image.width, height = image.height
    var pixels = Data(count: width * height * 4)
    let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
      guard let context = CGContext(data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                                    space: srgbSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
      else { return false }
      context.interpolationQuality = .high
      context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
      return true
    }
    guard drawn else { throw HelperError("Could not convert frame") }
    try pixels.write(to: url)
  }
}

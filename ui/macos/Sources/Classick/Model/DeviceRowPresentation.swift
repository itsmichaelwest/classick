import Foundation

struct DeviceRowPresentation: Equatable {
  enum Meter: Equatable {
    case capacity(used: UInt64, total: UInt64, projectedUsed: UInt64?)
    case progress(current: Int, total: Int, label: String?, etaSeconds: UInt64?)
    case indeterminate(label: String?)
    case unavailable
  }

  enum Action: Equatable {
    case syncNow
    case pause
    case cancel
    case resume
    case retry
    case details
    case setUp
  }

  var serial: DeviceID?
  var title: String
  var subtitle: String
  var caption: String?
  var meter: Meter
  var primaryAction: Action?
  var secondaryAction: Action?
  var accessibilityLabel: String = ""

  static func make(device: DeviceViewState?, libraryCount: Int?) -> Self {
    make(device: device, libraryCount: libraryCount, globalPhase: nil)
  }

  static func make(
    device: DeviceViewState?,
    libraryCount: Int?,
    globalPhase: Phase?
  ) -> Self {
    guard let device else {
      return makeWithoutDevice(libraryCount: libraryCount, globalPhase: globalPhase)
    }

    let title = DeviceIdentityLogic.title(identity: device.identity, hardware: device.hardware)
    let accessibilityLabel = DeviceIdentityLogic.accessibilityLabel(
      identity: device.identity, hardware: device.hardware)

    if device.finalization != nil {
      return Self(
        serial: device.deviceID,
        title: title,
        subtitle: "Finishing sync…",
        caption: "Keep the iPod connected",
        meter: .indeterminate(label: "Saving completed albums"),
        primaryAction: nil,
        secondaryAction: nil,
        accessibilityLabel: accessibilityLabel)
    }

    if let guidance = DeviceReadinessLogic.guidance(for: device.readiness) {
      return Self(
        serial: device.deviceID,
        title: title,
        subtitle: guidance.title,
        caption: guidance.message,
        meter: .unavailable,
        primaryAction: nil,
        secondaryAction: nil,
        accessibilityLabel: accessibilityLabel)
    }

    if case .scanning(let current, let total) = globalPhase {
      return Self(
        serial: device.deviceID,
        title: title,
        subtitle: "Updating library…",
        caption: nil,
        meter: .progress(
          current: current,
          total: total,
          label: "Scanning library",
          etaSeconds: nil),
        primaryAction: nil,
        secondaryAction: nil,
        accessibilityLabel: accessibilityLabel)
    }

    switch device.phase {
    case .disconnected:
      return Self(
        serial: device.deviceID,
        title: title,
        subtitle: "Not connected",
        caption: "Plug it in to sync",
        meter: .unavailable,
        primaryAction: nil,
        secondaryAction: nil,
        accessibilityLabel: accessibilityLabel)

    case .unconfigured:
      return Self(
        serial: device.deviceID,
        title: title,
        subtitle: "iPod not set up",
        caption: nil,
        meter: .unavailable,
        primaryAction: .setUp,
        secondaryAction: nil,
        accessibilityLabel: accessibilityLabel)

    case .idle:
      return Self(
        serial: device.deviceID,
        title: title,
        subtitle: lastSyncedLine(device.latestSuccessfulSync),
        caption: rollupCaption(device),
        meter: capacityMeter(device),
        primaryAction: .syncNow,
        secondaryAction: nil,
        accessibilityLabel: accessibilityLabel)

    case .syncing:
      guard let progress = device.syncProgress, progress.total > 0 else {
        return Self(
          serial: device.deviceID,
          title: title,
          subtitle: "Preparing sync…",
          caption: nil,
          meter: .indeterminate(label: "Preparing sync…"),
          primaryAction: .pause,
          secondaryAction: .cancel,
          accessibilityLabel: accessibilityLabel)
      }
      return Self(
        serial: device.deviceID,
        title: title,
        subtitle: "Adding \(progress.total) track\(progress.total == 1 ? "" : "s")",
        caption: nil,
        meter: .progress(
          current: progress.current,
          total: progress.total,
          label: progress.label.isEmpty ? nil : progress.label,
          etaSeconds: progress.etaSecs),
        primaryAction: .pause,
        secondaryAction: .cancel,
        accessibilityLabel: accessibilityLabel)

    case .paused:
      let total = device.libraryCount ?? libraryCount ?? 0
      let summary =
        total > 0
        ? "\(device.syncedCount) of \(total) synced"
        : "\(device.syncedCount) synced"
      return Self(
        serial: device.deviceID,
        title: title,
        subtitle: "Sync paused",
        caption: nil,
        meter: .progress(
          current: device.syncedCount,
          total: total,
          label: summary,
          etaSeconds: nil),
        primaryAction: .resume,
        secondaryAction: nil,
        accessibilityLabel: accessibilityLabel)

    case .error(let message):
      return Self(
        serial: device.deviceID,
        title: title,
        subtitle: "Sync failed",
        caption: message,
        meter: .unavailable,
        primaryAction: .retry,
        secondaryAction: .details,
        accessibilityLabel: accessibilityLabel)
    }
  }

  static func make(
    devices: [DeviceID: DeviceViewState],
    selectedSerial: DeviceID?,
    globalPhase: Phase,
    libraryCount: Int?
  ) -> Self {
    if let selectedSerial, let selected = devices[selectedSerial] {
      return make(device: selected, libraryCount: libraryCount, globalPhase: globalPhase)
    }

    let active = devices.values.filter { $0.sessionID != nil }
    if active.count == 1, let device = active.first {
      return make(device: device, libraryCount: libraryCount, globalPhase: globalPhase)
    }

    if devices.count == 1, let remembered = devices.values.first {
      return make(
        device: remembered,
        libraryCount: libraryCount,
        globalPhase: globalPhase)
    }

    if devices.count > 1 {
      var presentation = makeWithoutDevice(
        libraryCount: libraryCount,
        globalPhase: globalPhase)
      presentation.title = "\(devices.count) iPods available"
      presentation.subtitle = "Select an iPod to manage it"
      return presentation
    }

    return makeWithoutDevice(libraryCount: libraryCount, globalPhase: globalPhase)
  }

  private static func makeWithoutDevice(libraryCount: Int?, globalPhase: Phase?) -> Self {
    if case .scanning(let current, let total) = globalPhase {
      return Self(
        serial: nil,
        title: "No iPod connected",
        subtitle: "Updating library…",
        caption: libraryCaption(libraryCount),
        meter: .progress(
          current: current,
          total: total,
          label: "Scanning library",
          etaSeconds: nil),
        primaryAction: nil,
        secondaryAction: nil)
    }

    return Self(
      serial: nil,
      title: "No iPod connected",
      subtitle: "Plug in your iPod to sync",
      caption: libraryCaption(libraryCount),
      meter: .unavailable,
      primaryAction: nil,
      secondaryAction: nil,
      accessibilityLabel: "No iPod connected")
  }

  private static func libraryCaption(_ count: Int?) -> String {
    let count = count ?? 0
    return "\(count) track\(count == 1 ? "" : "s") selected"
  }

  private static func capacityMeter(_ device: DeviceViewState) -> Meter {
    guard let storage = device.storage, storage.total > 0 else { return .unavailable }
    let used = storage.total - min(storage.free, storage.total)
    let projectedUsed = device.preview?.projectedFreeBytes.map {
      storage.total - min($0, storage.total)
    }
    return .capacity(used: used, total: storage.total, projectedUsed: projectedUsed)
  }

  private static func rollupCaption(_ device: DeviceViewState) -> String? {
    let syncedSummary =
      device.libraryCount.map { "\(device.syncedCount) of \($0)" }
      ?? "\(device.syncedCount)"
    let caption = [
      DeviceRowFormatting.skippedForSpaceLine(
        syncedSummary: syncedSummary,
        skipped: device.lastRun?.skippedForSpace),
      DeviceRowFormatting.artworkMissingLine(device.lastRun?.artwork),
    ]
    .compactMap { $0 }
    .joined(separator: " · ")
    return caption.isEmpty ? nil : caption
  }

  private static func lastSyncedLine(_ entry: HistoryEntry?) -> String {
    guard let entry else { return "Never synced" }
    guard let date = ISO8601DateFormatter().date(from: entry.timestamp) else {
      return "Last synced at \(entry.timestamp)"
    }
    return "Last synced at \(date.formatted(date: .omitted, time: .shortened))"
  }
}

enum DeviceRowLayout {
  static let outerInset: CGFloat = 20
  static let cornerRadius: CGFloat = 16
  static let horizontalPadding: CGFloat = 16
  static let verticalPadding: CGFloat = 10
  static let artworkSize: CGFloat = 40
  static let headerToMeterSpacing: CGFloat = 12
  static let meterHeight: CGFloat = 6
  static let titleLineLimit = 1
  static let subtitleLineLimit = 1
  static let captionLineLimit = 1
}

enum DeviceRowFormatting {
  static func gbString(_ bytes: UInt64) -> String {
    String(format: "%.1f GB", Double(bytes) / 1_000_000_000)
  }

  static func skippedForSpaceLine(
    syncedSummary: String,
    skipped: SkippedForSpace?
  ) -> String? {
    guard let skipped, skipped.tracks > 0 else { return nil }
    return
      "Synced \(syncedSummary) — \(skipped.albums) album\(skipped.albums == 1 ? "" : "s") didn't fit (\(gbString(skipped.bytes)))"
  }

  static func artworkMissingCount(_ artwork: ArtworkSummary?) -> Int? {
    guard let artwork else { return nil }
    let shortfall = artwork.eligible - artwork.embedded
    guard artwork.failedSources > 0 || shortfall > 0 else { return nil }
    return shortfall > 0 ? shortfall : artwork.failedSources
  }

  static func artworkMissingLine(_ artwork: ArtworkSummary?) -> String? {
    guard let count = artworkMissingCount(artwork) else { return nil }
    return "Art missing for \(count) track\(count == 1 ? "" : "s")"
  }
}

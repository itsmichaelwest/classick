import SwiftUI

struct DeviceRow: View {
  var model: AppModel
  var selectedSerial: DeviceSerial?
  var onSyncNow: (DeviceSerial) -> Void
  var onPause: (DeviceSerial) -> Void
  var onCancelSync: (DeviceSerial) -> Void
  var onResume: (DeviceSerial) -> Void
  var onRetry: (DeviceSerial) -> Void
  var onSetUp: (DeviceSerial?) -> Void

  private var presentation: DeviceRowPresentation {
    DeviceRowPresentation.make(
      devices: model.devices,
      selectedSerial: selectedSerial,
      globalPhase: model.phase,
      libraryCount: model.libraryCount ?? model.library?.totalTracks)
  }

  private var drive: String? {
    presentation.serial.flatMap { model.devices[$0]?.mountPath }
  }

  var body: some View {
    VStack(alignment: .leading, spacing: DeviceRowLayout.headerToMeterSpacing) {
      header
      meter
    }
    .padding(.horizontal, DeviceRowLayout.horizontalPadding)
    .padding(.vertical, DeviceRowLayout.verticalPadding)
    .frame(maxWidth: .infinity)
    .floatingBarBackground()
    .padding(.horizontal, DeviceRowLayout.outerInset)
    .padding(.bottom, DeviceRowLayout.outerInset)
  }

  private var header: some View {
    HStack(spacing: 12) {
      DeviceIcon(
        serial: presentation.serial,
        drive: drive,
        size: DeviceRowLayout.artworkSize)

      VStack(alignment: .leading, spacing: 2) {
        Text(presentation.title)
          .font(.title3.bold())
          .lineLimit(DeviceRowLayout.titleLineLimit)
          .truncationMode(.tail)
        Text(presentation.subtitle)
          .foregroundStyle(subtitleStyle)
          .lineLimit(DeviceRowLayout.subtitleLineLimit)
          .truncationMode(.tail)
      }
      .layoutPriority(1)

      Spacer(minLength: 8)
      actions
    }
  }

  private var subtitleStyle: AnyShapeStyle {
    presentation.primaryAction == .retry ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary)
  }

  @ViewBuilder
  private var actions: some View {
    HStack(spacing: 8) {
      if let action = presentation.secondaryAction {
        actionButton(action, prominent: false)
      }
      if let action = presentation.primaryAction {
        actionButton(action, prominent: true)
      }
    }
    .fixedSize()
  }

  @ViewBuilder
  private func actionButton(_ action: DeviceRowPresentation.Action, prominent: Bool) -> some View {
    if prominent {
      Button(role: action == .cancel ? .destructive : nil) {
        perform(action)
      } label: {
        Text(actionLabel(action))
      }
      .buttonStyle(.borderedProminent)
      .controlSize(.large)
    } else {
      Button(role: action == .cancel ? .destructive : nil) {
        perform(action)
      } label: {
        Text(actionLabel(action))
      }
      .buttonStyle(.bordered)
      .controlSize(.large)
    }
  }

  private var meter: some View {
    VStack(alignment: .leading, spacing: 6) {
      meterBar
        .frame(height: DeviceRowLayout.meterHeight)
      meterCaption
        .frame(maxWidth: .infinity, minHeight: 16, maxHeight: 16)
    }
  }

  @ViewBuilder
  private var meterBar: some View {
    switch presentation.meter {
    case .capacity(let used, let total, let projectedUsed):
      GeometryReader { proxy in
        let usedFraction = fraction(used, total: total)
        let projectedFraction = projectedUsed.map { fraction($0, total: total) }
        ZStack(alignment: .leading) {
          Capsule().fill(.quaternary)
          if let projectedFraction, projectedFraction > usedFraction {
            Capsule()
              .fill(.orange.opacity(0.55))
              .frame(width: proxy.size.width * projectedFraction)
          }
          Capsule()
            .fill(Color.accentColor)
            .frame(width: proxy.size.width * usedFraction)
        }
      }

    case .progress(let current, let total, _, _):
      ProgressView(value: progressValue(current: current, total: total))
        .progressViewStyle(.linear)
        .tint(.accentColor)

    case .indeterminate:
      ProgressView()
        .progressViewStyle(.linear)

    case .unavailable:
      Capsule().fill(.quaternary)
    }
  }

  @ViewBuilder
  private var meterCaption: some View {
    switch presentation.meter {
    case .capacity(let used, let total, _):
      HStack(spacing: 8) {
        Text(capacityCaption(used: used))
          .lineLimit(DeviceRowLayout.captionLineLimit)
          .truncationMode(.middle)
        Spacer(minLength: 8)
        Text("\(formatBytes(total)) total")
          .fixedSize()
      }
      .font(.callout)
      .foregroundStyle(.secondary)

    case .progress(let current, let total, let label, let etaSeconds):
      HStack(spacing: 8) {
        Text(progressCaption(current: current, total: total, label: label))
          .lineLimit(DeviceRowLayout.captionLineLimit)
          .truncationMode(.middle)
        Spacer(minLength: 8)
        if let etaSeconds {
          Text("~\(formatEta(etaSeconds)) left")
            .fixedSize()
        }
      }
      .font(.callout)
      .foregroundStyle(.secondary)

    case .indeterminate(let label):
      HStack(spacing: 8) {
        Text(label ?? presentation.caption ?? " ")
          .lineLimit(DeviceRowLayout.captionLineLimit)
          .truncationMode(.tail)
        Spacer(minLength: 8)
        if label != nil, let caption = presentation.caption {
          Text(caption)
            .lineLimit(DeviceRowLayout.captionLineLimit)
            .fixedSize()
        }
      }
      .font(.callout)
      .foregroundStyle(.secondary)

    case .unavailable:
      Text(presentation.caption ?? " ")
        .font(.callout)
        .foregroundStyle(.secondary)
        .lineLimit(DeviceRowLayout.captionLineLimit)
        .truncationMode(.middle)
    }
  }

  private func capacityCaption(used: UInt64) -> String {
    let usage = "\(formatBytes(used)) used"
    guard let caption = presentation.caption else { return usage }
    return "\(usage) · \(caption)"
  }

  private func progressCaption(current: Int, total: Int, label: String?) -> String {
    if let label, label.hasPrefix("\(current) ") {
      return label
    }
    let progress = total > 0 ? "\(current) of \(total)" : "Preparing…"
    guard let label, !label.isEmpty else { return progress }
    return "\(progress) · \(label)"
  }

  private func fraction(_ value: UInt64, total: UInt64) -> Double {
    guard total > 0 else { return 0 }
    return min(1, max(0, Double(value) / Double(total)))
  }

  private func progressValue(current: Int, total: Int) -> Double {
    guard total > 0 else { return 0 }
    return min(1, max(0, Double(current) / Double(total)))
  }

  private func actionLabel(_ action: DeviceRowPresentation.Action) -> String {
    switch action {
    case .syncNow: "Sync Now"
    case .pause: "Pause"
    case .cancel: "Cancel"
    case .resume: "Resume"
    case .retry: "Retry"
    case .details: "Details"
    case .setUp: "Set Up…"
    }
  }

  private func perform(_ action: DeviceRowPresentation.Action) {
    switch action {
    case .setUp:
      onSetUp(presentation.serial)
    case .details:
      guard let serial = presentation.serial else { return }
      model.dismissTerminalError(for: serial)
      model.selectedDestination = .history
    case .syncNow:
      withSerial(onSyncNow)
    case .pause:
      withSerial(onPause)
    case .cancel:
      withSerial(onCancelSync)
    case .resume:
      withSerial(onResume)
    case .retry:
      withSerial(onRetry)
    }
  }

  private func withSerial(_ action: (DeviceSerial) -> Void) {
    guard let serial = presentation.serial else { return }
    action(serial)
  }

  private func formatEta(_ seconds: UInt64) -> String {
    let formatter = DateComponentsFormatter()
    formatter.allowedUnits = seconds < 3600 ? [.minute, .second] : [.hour, .minute]
    formatter.unitsStyle = .abbreviated
    return formatter.string(from: TimeInterval(seconds)) ?? "\(seconds)s"
  }
}

extension View {
  @ViewBuilder
  fileprivate func floatingBarBackground() -> some View {
    if #available(macOS 26.0, *) {
      glassEffect(
        .regular,
        in: RoundedRectangle(
          cornerRadius: DeviceRowLayout.cornerRadius,
          style: .continuous))
    } else {
      background(
        .regularMaterial,
        in: RoundedRectangle(
          cornerRadius: DeviceRowLayout.cornerRadius,
          style: .continuous)
      )
      .overlay(
        RoundedRectangle(
          cornerRadius: DeviceRowLayout.cornerRadius,
          style: .continuous
        )
        .strokeBorder(Color(nsColor: .separatorColor).opacity(0.6), lineWidth: 1)
      )
      .shadow(color: .black.opacity(0.12), radius: 10, y: 3)
    }
  }
}

#if DEBUG
  @MainActor
  private func deviceRowPreview(
    _ model: AppModel,
    width: CGFloat,
    colorScheme: ColorScheme
  ) -> some View {
    DeviceRow(
      model: model,
      selectedSerial: {
        guard case .device(let serial, _) = model.selectedDestination else { return nil }
        return serial
      }(),
      onSyncNow: { _ in },
      onPause: { _ in },
      onCancelSync: { _ in },
      onResume: { _ in },
      onRetry: { _ in },
      onSetUp: { _ in }
    )
    .frame(width: width)
    .preferredColorScheme(colorScheme)
  }

  #Preview("600 · Light · Long content") {
    deviceRowPreview(PreviewFixtures.longContentErrorModel(), width: 600, colorScheme: .light)
  }

  #Preview("600 · Dark · Long content") {
    deviceRowPreview(PreviewFixtures.longContentErrorModel(), width: 600, colorScheme: .dark)
  }

  #Preview("820 · Light · Long content") {
    deviceRowPreview(PreviewFixtures.longContentErrorModel(), width: 820, colorScheme: .light)
  }

  #Preview("820 · Dark · Long content") {
    deviceRowPreview(PreviewFixtures.longContentErrorModel(), width: 820, colorScheme: .dark)
  }

  #Preview("860 · Light · Long content") {
    deviceRowPreview(PreviewFixtures.longContentErrorModel(), width: 860, colorScheme: .light)
  }

  #Preview("860 · Dark · Long content") {
    deviceRowPreview(PreviewFixtures.longContentErrorModel(), width: 860, colorScheme: .dark)
  }

  #Preview("Finalizing") {
    deviceRowPreview(PreviewFixtures.finalizingModel(), width: 820, colorScheme: .light)
  }

  #Preview("Idle") {
    deviceRowPreview(PreviewFixtures.connectedSyncedModel(), width: 820, colorScheme: .light)
  }

  #Preview("Syncing") {
    deviceRowPreview(PreviewFixtures.syncingModel(), width: 820, colorScheme: .light)
  }

  #Preview("Paused") {
    deviceRowPreview(PreviewFixtures.pausedModel(), width: 820, colorScheme: .light)
  }

  #Preview("Scanning") {
    deviceRowPreview(PreviewFixtures.scanningModel(), width: 820, colorScheme: .light)
  }

  #Preview("Disconnected") {
    deviceRowPreview(PreviewFixtures.disconnectedModel(), width: 820, colorScheme: .light)
  }

  #Preview("Not configured") {
    deviceRowPreview(PreviewFixtures.notConfiguredModel(), width: 820, colorScheme: .light)
  }
#endif

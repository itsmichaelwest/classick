import SwiftUI

struct DeviceIcon: View {
  var hardware: WireV3Hardware
  var size: CGFloat

  var body: some View {
    Group {
      if case .exact(let resourceName) = DeviceIconLogic.artwork(for: hardware),
        let icon = NSImage(
          contentsOfFile: "\(DeviceIconLogic.ampResourcesDir)/\(resourceName).icns")
      {
        Image(nsImage: icon)
          .resizable()
          .interpolation(.high)
          .scaledToFit()
      } else if case .generic(let token) = DeviceIconLogic.artwork(for: hardware) {
        genericArtwork(token)
      }
    }
    .frame(width: size, height: size)
    .accessibilityHidden(true)
  }

  private func genericArtwork(_ token: GenericDeviceArtwork) -> some View {
    ZStack(alignment: .bottomTrailing) {
      Image(systemName: token.baseSystemImage)
        .font(.system(size: size * 0.8))
      if let badge = token.badgeSystemImage {
        Image(systemName: badge)
          .font(.system(size: max(8, size * 0.3), weight: .semibold))
          .symbolRenderingMode(.palette)
          .foregroundStyle(Color.primary, Color(nsColor: .windowBackgroundColor))
      }
    }
    .foregroundStyle(.secondary)
  }
}

enum DeviceArtwork: Equatable, Sendable {
  case exact(resourceName: String)
  case generic(GenericDeviceArtwork)
}

enum GenericDeviceArtwork: Equatable, Sendable {
  case classic
  case nano
  case mini
  case shuffle(generation: Int?)
  case video
  case photo
  case touch
  case ipod
  case unknown

  var baseSystemImage: String {
    switch self {
    case .shuffle(let generation):
      generation.map { "ipodshuffle.gen\($0)" } ?? "square.roundedrectangle"
    case .touch:
      "ipodtouch"
    default:
      "ipod"
    }
  }

  var badgeSystemImage: String? {
    switch self {
    case .classic: "circle.circle.fill"
    case .nano: "n.circle.fill"
    case .mini: "m.circle.fill"
    case .shuffle(nil): "shuffle.circle.fill"
    case .video: "play.circle.fill"
    case .photo: "photo.circle.fill"
    case .unknown: "questionmark.circle.fill"
    default: nil
    }
  }
}

enum DeviceIconLogic {
  nonisolated static let ampResourcesDir =
    "/System/Library/PrivateFrameworks/AMPDevices.framework/Versions/A/Resources"

  /// Exact Finder-style artwork is allowed only when the daemon supplied a
  /// certain model code and independently decoded its colour. Swift never
  /// guesses colour from capacity, names, mount paths, or persisted settings.
  nonisolated static func artwork(for hardware: WireV3Hardware) -> DeviceArtwork {
    guard let family = hardware.family,
      family.value.lowercased() == "classic",
      family.source == "decoded",
      family.confidence == "certain",
      let model = hardware.modelCode,
      model.confidence == "certain",
      model.source == "reported" || model.source == "decoded",
      let colour = hardware.colour,
      colour.confidence == "certain",
      colour.source == "decoded"
    else { return .generic(genericArtwork(for: hardware)) }

    let key = "\(model.value.uppercased())|\(colour.value.lowercased())"
    switch key {
    case "MB029|silver", "MB145|silver", "MB562|silver", "MC293|silver":
      return .exact(resourceName: "iPod11-Silver")
    case "MB147|black", "MB150|black", "MB565|black":
      return .exact(resourceName: "iPod11-Black")
    case "MC297|black":
      return .exact(resourceName: "iPod11B-Black")
    default:
      return .generic(genericArtwork(for: hardware))
    }
  }

  nonisolated static func genericArtwork(for hardware: WireV3Hardware) -> GenericDeviceArtwork {
    guard let family = hardware.family,
      family.confidence == "certain",
      family.source == "decoded" || family.source == "reported"
    else { return .unknown }

    switch family.value.lowercased() {
    case "classic": return .classic
    case "nano": return .nano
    case "mini": return .mini
    case "shuffle": return .shuffle(generation: deterministicGeneration(hardware.generation))
    case "video": return .video
    case "photo": return .photo
    case "touch": return .touch
    case "ipod": return .ipod
    default: return .unknown
    }
  }

  private nonisolated static func deterministicGeneration(
    _ fact: WireV3HardwareFact<String>?
  ) -> Int? {
    guard let fact,
      fact.confidence == "certain",
      fact.source == "decoded" || fact.source == "reported",
      let value = Int(fact.value), (1...4).contains(value)
    else { return nil }
    return value
  }

  nonisolated static let allExactResourceNames: Set<String> = [
    "iPod11-Silver", "iPod11-Black", "iPod11B-Black",
  ]
}

#if DEBUG
  #Preview("Exact decoded artwork") {
    DeviceIcon(hardware: PreviewFixtures.exactClassicHardware, size: 80)
      .padding()
  }

  #Preview("Generic model artwork") {
    DeviceIcon(hardware: PreviewFixtures.genericClassicHardware, size: 80)
      .padding()
  }

  #Preview("Generic family artwork") {
    HStack(spacing: 24) {
      ForEach(
        [("nano", nil), ("mini", nil), ("shuffle", "3"), ("video", nil), ("touch", nil)],
        id: \.0
      ) { family, generation in
        VStack {
          DeviceIcon(
            hardware: PreviewFixtures.genericHardware(
              family: family, generation: generation),
            size: 64)
          Text(family.capitalized)
            .font(.caption)
        }
      }
    }
    .padding()
  }
#endif

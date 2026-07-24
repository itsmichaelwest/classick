import SwiftUI

struct DeviceIcon: View {
  var hardware: WireV3Hardware
  var size: CGFloat
  var serial: DeviceID?
  private var cache: DeviceArtworkCache

  init(
    hardware: WireV3Hardware,
    size: CGFloat,
    serial: DeviceID? = nil,
    cache: DeviceArtworkCache = DeviceArtworkCache()
  ) {
    self.hardware = hardware
    self.size = size
    self.serial = serial
    self.cache = cache
  }

  var body: some View {
    Group {
      if let icon = DeviceIconLogic.image(for: resolvedArtwork) {
        Image(nsImage: icon)
          .resizable()
          .interpolation(.high)
          .scaledToFit()
      } else {
        Image(systemName: "ipod")
          .resizable()
          .scaledToFit()
          .foregroundStyle(.secondary)
      }
    }
    .frame(width: size, height: size)
    .accessibilityHidden(true)
    .onAppear(perform: rememberExactArtwork)
    .onChange(of: hardware) { _, _ in rememberExactArtwork() }
  }

  private var resolvedArtwork: DeviceArtwork {
    DeviceIconLogic.resolvedArtwork(for: hardware, serial: serial, cache: cache)
  }

  private func rememberExactArtwork() {
    guard let serial else { return }
    cache.rememberExactArtwork(for: hardware, serial: serial)
  }

}

struct DeviceArtworkCache {
  private static let storageKey = "deviceArtworkResources.v2"
  private let defaults: UserDefaults

  init(defaults: UserDefaults = .standard) {
    self.defaults = defaults
  }

  func resourceName(for serial: DeviceID) -> String? {
    guard
      let resourceName =
        (defaults.dictionary(forKey: Self.storageKey) as? [String: String])?[serial.rawValue],
      DeviceIconLogic.allExactResourceNames.contains(resourceName)
    else { return nil }
    return resourceName
  }

  func rememberExactArtwork(for hardware: WireV3Hardware, serial: DeviceID) {
    guard case .exact(let resourceName) = DeviceIconLogic.artwork(for: hardware),
      DeviceIconLogic.allExactResourceNames.contains(resourceName)
    else { return }
    var resources =
      defaults.dictionary(forKey: Self.storageKey) as? [String: String] ?? [:]
    guard resources[serial.rawValue] != resourceName else { return }
    resources[serial.rawValue] = resourceName
    defaults.set(resources, forKey: Self.storageKey)
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
}

enum DeviceIconLogic {
  nonisolated static let ampFrameworkPath =
    "/System/Library/PrivateFrameworks/AMPDevices.framework"
  nonisolated static let ampResourcesDir =
    "/System/Library/PrivateFrameworks/AMPDevices.framework/Versions/A/Resources"

  nonisolated static func frameworkImage(named resourceName: String) -> NSImage? {
    Bundle(path: ampFrameworkPath)?.image(forResource: NSImage.Name(resourceName))
  }

  nonisolated static func image(for artwork: DeviceArtwork) -> NSImage? {
    let requestedName: String? = switch artwork {
    case .exact(let resourceName): resourceName
    case .generic: nil
    }
    return requestedName.flatMap(frameworkImage(named:))
      ?? frameworkImage(named: genericResourceName)
  }

  /// Exact Finder-style artwork is allowed only when the daemon supplied a
  /// certain model code and independently decoded its colour. Swift never
  /// guesses colour from capacity, names, mount paths, or persisted settings.
  nonisolated static func artwork(for hardware: WireV3Hardware) -> DeviceArtwork {
    guard let family = hardware.family,
      family.source == "decoded",
      family.confidence == "certain",
      let generation = hardware.generation,
      generation.source == "decoded",
      generation.confidence == "certain",
      let model = hardware.modelCode,
      model.confidence == "certain",
      model.source == "reported" || model.source == "decoded",
      let colour = hardware.colour,
      colour.confidence == "certain",
      colour.source == "decoded"
    else { return .generic(genericArtwork(for: hardware)) }

    let colourName = frameworkColourName(colour.value)
    let resourceName: String? = switch (family.value.lowercased(), generation.value) {
    case ("ipod", "1"), ("ipod", "2"): "iPod1"
    case ("ipod", "3"): "iPod2"
    case ("ipod", "4"): colourName.map { "iPod4-\($0)" }
    case ("photo", _): colourName.map { "iPod5-\($0)" }
    case ("mini", "1"): colourName.map { "iPod3-\($0)" }
    case ("mini", "2") where colourName == "Silver": "iPod3-Silver"
    case ("mini", "2"): colourName.map { "iPod3B-\($0)" }
    case ("video", "5"), ("video", "5.5"): colourName.map { "iPodM25-\($0)" }
    case ("nano", "1"): colourName.map { "iPodM26-\($0)" }
    case ("nano", "2"): colourName.map { "iPod9-\($0)" }
    case ("nano", "3"): colourName.map { "iPod12-\($0)" }
    case ("nano", "4"): colourName.map { "iPod15-\($0)" }
    case ("classic", "1"), ("classic", "2"): colourName.map { "iPodN25-\($0)" }
    case ("classic", "3"): colourName.map { "iPodN25B-\($0)" }
    default: nil
    }
    guard let resourceName, allExactResourceNames.contains(resourceName) else {
      return .generic(genericArtwork(for: hardware))
    }
    return .exact(resourceName: resourceName)
  }

  nonisolated static func resolvedArtwork(
    for hardware: WireV3Hardware,
    serial: DeviceID?,
    cache: DeviceArtworkCache
  ) -> DeviceArtwork {
    let observed = artwork(for: hardware)
    if case .exact = observed {
      return observed
    }
    if let serial, let resourceName = cache.resourceName(for: serial) {
      return .exact(resourceName: resourceName)
    }
    return .exact(resourceName: genericResourceName)
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

  private nonisolated static func frameworkColourName(_ value: String) -> String? {
    switch value.lowercased() {
    case "silver": "Silver"
    case "black": "Black"
    case "black_red": "BlackRed"
    case "white": "White"
    case "blue": "Blue"
    case "green": "Green"
    case "pink": "Pink"
    case "red": "Red"
    case "yellow": "Yellow"
    case "purple": "Purple"
    case "orange": "Orange"
    case "gold": "Gold"
    default: nil
    }
  }

  nonisolated static let allExactResourceNames: Set<String> = [
    "iPod1", "iPod2",
    "iPod3-Silver", "iPod3-Blue", "iPod3-Green", "iPod3-Pink", "iPod3-Gold",
    "iPod3B-Blue", "iPod3B-Green", "iPod3B-Pink",
    "iPod4-White", "iPod4-BlackRed",
    "iPod5-White", "iPod5-BlackRed",
    "iPodM25-White", "iPodM25-Black", "iPodM25-BlackRed",
    "iPodM26-White", "iPodM26-Black",
    "iPod9-Silver", "iPod9-Black", "iPod9-Blue", "iPod9-Green", "iPod9-Pink", "iPod9-Red",
    "iPod12-Silver", "iPod12-Black", "iPod12-Blue", "iPod12-Green", "iPod12-Pink",
    "iPod12-Red",
    "iPod15-Silver", "iPod15-Black", "iPod15-Blue", "iPod15-Green", "iPod15-Pink",
    "iPod15-Red", "iPod15-Yellow", "iPod15-Purple", "iPod15-Orange",
    "iPodN25-Silver", "iPodN25-Black",
    "iPodN25B-Silver", "iPodN25B-Black",
  ]
  nonisolated static let genericResourceName = "iPodGeneric"
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

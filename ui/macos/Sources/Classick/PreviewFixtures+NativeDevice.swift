#if DEBUG
  import Foundation

  extension PreviewFixtures {
    static let nativeDeviceID: DeviceID = "000A27002138B0A8"

    static func nativeDeviceModel(
      name: String? = "Michael West's iPod",
      readiness: String = "ready",
      hardware: WireV3Hardware = exactClassicHardware,
      connected: Bool = true,
      configured: Bool = true
    ) -> AppModel {
      let model = AppModel()
      model.apply(.hello(protocolVersion: "3.0.0", coreVersion: "preview"))
      model.apply(
        .deviceInventory(
          WireV3DeviceInventory(
            requestID: nil,
            revision: 1,
            devices: [
              WireV3IdentifiedDevice(
                deviceID: nativeDeviceID,
                name: name,
                readiness: readiness,
                hardware: hardware,
                profileStatus: configured ? "adopted" : "not_adopted",
                connected: connected,
                mountPath: connected ? "/Volumes/iPod" : nil,
                phase: connected ? (configured ? "idle" : "unconfigured") : "disconnected",
                sessionID: nil,
                storage: connected
                  ? WireV3Storage(
                    totalBytes: 160_000_000_000, freeBytes: 110_000_000_000,
                    freshness: "fresh") : nil,
                syncedCount: 42,
                libraryCount: 91,
                lastTerminalError: nil)
            ],
            unidentified: [])))
      return model
    }

    static func unidentifiedDeviceModel() -> AppModel {
      let model = AppModel()
      model.apply(.hello(protocolVersion: "3.0.0", coreVersion: "preview"))
      model.apply(
        .deviceInventory(
          WireV3DeviceInventory(
            requestID: nil,
            revision: 1,
            devices: [],
            unidentified: [
              WireV3UnidentifiedDevice(
                observationID: try! ObservationID(1),
                readiness: "identity_unavailable",
                hardware: genericClassicHardware)
            ])))
      return model
    }

    static let exactClassicHardware = WireV3Hardware(
      family: fact("classic"),
      generation: fact("3"),
      modelCode: fact("MC293", source: "reported"),
      colour: fact("silver"),
      firmware: fact("2.0.4", source: "reported"),
      capacityBytes: fact(160_000_000_000, source: "reported"))

    static let genericClassicHardware = WireV3Hardware(
      family: fact("classic"),
      generation: fact("3"),
      modelCode: nil,
      colour: nil,
      firmware: fact("2.0.4", source: "reported"),
      capacityBytes: fact(160_000_000_000, source: "reported"))

    static func genericHardware(family: String, generation: String? = nil) -> WireV3Hardware {
      WireV3Hardware(
        family: fact(family),
        generation: generation.map { fact($0) },
        modelCode: nil, colour: nil, firmware: nil, capacityBytes: nil)
    }

    private static func fact<T: Codable & Equatable & Sendable>(
      _ value: T, source: String = "decoded"
    ) -> WireV3HardwareFact<T> {
      .init(value: value, source: source, confidence: "certain")
    }
  }
#endif

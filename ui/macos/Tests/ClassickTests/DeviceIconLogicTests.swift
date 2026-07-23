import XCTest

@testable import Classick

final class DeviceIconLogicTests: XCTestCase {
  func testUnverifiedArtworkUsesAMPDevicesGenericResource() throws {
    let (cache, cleanup) = try isolatedCache()
    defer { cleanup() }

    XCTAssertEqual(
      DeviceIconLogic.resolvedArtwork(
        for: hardware(), serial: DeviceID("0000000000000ABC"), cache: cache),
      .exact(resourceName: "iPodGeneric"))
  }

  func testVerifiedExactArtworkSurvivesMissingDisconnectedFacts() throws {
    let (cache, cleanup) = try isolatedCache()
    defer { cleanup() }
    let serial = DeviceID("0000000000000ABC")
    let exact = hardware(
      model: fact("MC297", source: "reported"),
      colour: fact("black"))

    cache.rememberExactArtwork(for: exact, serial: serial)

    XCTAssertEqual(
      DeviceIconLogic.resolvedArtwork(for: hardware(), serial: serial, cache: cache),
      .exact(resourceName: "iPod11B-Black"))
  }

  func testGenericObservationNeverOverwritesCachedExactArtwork() throws {
    let (cache, cleanup) = try isolatedCache()
    defer { cleanup() }
    let serial = DeviceID("0000000000000ABC")
    cache.rememberExactArtwork(
      for: hardware(
        model: fact("MC293", source: "reported"),
        colour: fact("silver")),
      serial: serial)

    cache.rememberExactArtwork(for: hardware(), serial: serial)

    XCTAssertEqual(cache.resourceName(for: serial), "iPod11-Silver")
  }

  func testCertainReportedModelAndDecodedColourSelectExactArtwork() {
    XCTAssertEqual(
      DeviceIconLogic.artwork(
        for: hardware(model: fact("MC293", source: "reported"), colour: fact("silver"))),
      .exact(resourceName: "iPod11-Silver"))
    XCTAssertEqual(
      DeviceIconLogic.artwork(
        for: hardware(model: fact("MC297", source: "reported"), colour: fact("black"))),
      .exact(resourceName: "iPod11B-Black"))
  }

  func testExactArtworkRequiresIndependentDecodedColour() {
    XCTAssertEqual(
      DeviceIconLogic.artwork(
        for: hardware(
          model: fact("MC297", source: "reported"),
          colour: fact("black", source: "reported"))),
      .generic(.classic))
    XCTAssertEqual(
      DeviceIconLogic.artwork(
        for: hardware(
          model: fact("MC297", source: "reported"),
          colour: fact("black", confidence: "heuristic"))),
      .generic(.classic))
  }

  func testUnknownMissingAndMismatchedFactsUseGenericArtwork() {
    XCTAssertEqual(DeviceIconLogic.artwork(for: hardware()), .generic(.classic))
    XCTAssertEqual(
      DeviceIconLogic.artwork(
        for: hardware(model: fact("UNKNOWN", source: "reported"), colour: fact("black"))),
      .generic(.classic))
    XCTAssertEqual(
      DeviceIconLogic.artwork(
        for: hardware(model: fact("MC293", source: "reported"), colour: fact("black"))),
      .generic(.classic))
  }

  func testConflictingOrInferredFamilyCannotSelectClassicArtwork() {
    let exactModel = fact("MC293", source: "reported")
    let exactColour = fact("silver")
    let nano = WireV3Hardware(
      family: fact("nano"), generation: fact("3"), modelCode: exactModel,
      colour: exactColour, firmware: nil, capacityBytes: nil)
    XCTAssertEqual(DeviceIconLogic.artwork(for: nano), .generic(.nano))

    let inferred = WireV3Hardware(
      family: fact("classic", source: "inferred", confidence: "heuristic"),
      generation: fact("3"), modelCode: exactModel, colour: exactColour,
      firmware: nil, capacityBytes: nil)
    XCTAssertEqual(DeviceIconLogic.artwork(for: inferred), .generic(.unknown))
  }

  func testInferredStorageAndFamilyNeverBecomeExactArtwork() {
    let facts = WireV3Hardware(
      family: fact("classic", source: "decoded"),
      generation: fact("1", source: "inferred", confidence: "heuristic"),
      modelCode: nil, colour: nil, firmware: nil,
      capacityBytes: fact(80_000_000_000, source: "reported"))
    XCTAssertEqual(DeviceIconLogic.artwork(for: facts), .generic(.classic))
  }

  func testGenericArtworkUsesEveryDeterministicFamilyAndHonestShuffleGeneration() {
    let expected: [(String, GenericDeviceArtwork)] = [
      ("classic", .classic), ("nano", .nano), ("mini", .mini),
      ("video", .video), ("photo", .photo), ("touch", .touch), ("ipod", .ipod),
    ]
    for (family, token) in expected {
      let facts = WireV3Hardware(
        family: fact(family), generation: nil, modelCode: nil, colour: nil,
        firmware: nil, capacityBytes: nil)
      XCTAssertEqual(DeviceIconLogic.artwork(for: facts), .generic(token), family)
    }

    let knownShuffle = WireV3Hardware(
      family: fact("shuffle"), generation: fact("3"), modelCode: nil, colour: nil,
      firmware: nil, capacityBytes: nil)
    XCTAssertEqual(DeviceIconLogic.artwork(for: knownShuffle), .generic(.shuffle(generation: 3)))

    let inferredShuffle = WireV3Hardware(
      family: fact("shuffle"),
      generation: fact("3", source: "inferred", confidence: "heuristic"),
      modelCode: nil, colour: nil, firmware: nil, capacityBytes: nil)
    XCTAssertEqual(
      DeviceIconLogic.artwork(for: inferredShuffle), .generic(.shuffle(generation: nil)))
  }

  func testAllExactIconsExistOnThisSystem() throws {
    try XCTSkipUnless(
      FileManager.default.fileExists(atPath: DeviceIconLogic.ampResourcesDir),
      "AMPDevices resources not present on this OS — runtime uses the generic symbol")
    for name in DeviceIconLogic.allExactResourceNames.sorted() {
      XCTAssertTrue(
        FileManager.default.fileExists(atPath: "\(DeviceIconLogic.ampResourcesDir)/\(name).icns"),
        "missing icon resource: \(name).icns")
    }
    XCTAssertTrue(
      FileManager.default.fileExists(
        atPath: "\(DeviceIconLogic.ampResourcesDir)/iPodGeneric.icns"))
  }

  private func isolatedCache() throws -> (DeviceArtworkCache, () -> Void) {
    let suiteName = "DeviceIconLogicTests.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
    defaults.removePersistentDomain(forName: suiteName)
    return (
      DeviceArtworkCache(defaults: defaults),
      { defaults.removePersistentDomain(forName: suiteName) })
  }

  private func hardware(
    model: WireV3HardwareFact<String>? = nil,
    colour: WireV3HardwareFact<String>? = nil
  ) -> WireV3Hardware {
    WireV3Hardware(
      family: fact("classic"), generation: fact("3"), modelCode: model, colour: colour,
      firmware: nil, capacityBytes: nil)
  }

  private func fact<T: Codable & Equatable & Sendable>(
    _ value: T, source: String = "decoded", confidence: String = "certain"
  ) -> WireV3HardwareFact<T> {
    .init(value: value, source: source, confidence: confidence)
  }
}

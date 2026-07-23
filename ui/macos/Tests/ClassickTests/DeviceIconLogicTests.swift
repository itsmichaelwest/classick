import XCTest

@testable import Classick

/// `DeviceIconLogic` (device row icon): parsing the board-reported
/// `ModelNumStr` out of `iPod_Control/Device/SysInfo` and the
/// libgpod-derived model → Finder-icon table (see the enum's doc comment;
/// the table mirrors itdb_device.c's `ipod_model_table` at the vendored
/// commit).
final class DeviceIconLogicTests: XCTestCase {
  // MARK: - SysInfo parse (→ libgpod's 4-char table form, leading M stripped)

  func testParsesModelNumFromRealSysInfoShape() {
    let sysInfo = """
      BoardHwName: iPod Q
      pszSerialNumber: 000A27002138B0A8
      ModelNumStr: MC293
      buildID: 0x061710B3
      """
    XCTAssertEqual(DeviceIconLogic.parseModelNum(sysInfo: sysInfo), "C293")
  }

  func testStripsFirmwareXPrefixAndRegionSuffix() {
    XCTAssertEqual(DeviceIconLogic.parseModelNum(sysInfo: "ModelNumStr: xMC297ZP/A"), "C297")
  }

  func testUppercasesAndNormalizesModelNum() {
    XCTAssertEqual(DeviceIconLogic.parseModelNum(sysInfo: "ModelNumStr: mb565"), "B565")
  }

  func testFirstGenNumericModelSurvivesMStrip() {
    XCTAssertEqual(DeviceIconLogic.parseModelNum(sysInfo: "ModelNumStr: M8541"), "8541")
  }

  func testMissingModelNumLineIsNil() {
    XCTAssertNil(DeviceIconLogic.parseModelNum(sysInfo: "BoardHwName: iPod Q\n"))
  }

  func testEmptyOrTruncatedValueIsNil() {
    XCTAssertNil(DeviceIconLogic.parseModelNum(sysInfo: "ModelNumStr: \n"))
    XCTAssertNil(DeviceIconLogic.parseModelNum(sysInfo: "ModelNumStr: M12\n"))
  }

  // MARK: - Model → icon mapping (one representative per family)

  func testEveryFamilyMapsToItsIcon() {
    let expectations: [(String, String)] = [
      ("8541", "iPod1"),  // 1G scroll wheel
      ("8738", "iPod1"),  // 2G touch wheel
      ("9244", "iPod2"),  // 3G dock connector
      ("9282", "iPod4-White"),  // 4G mono
      ("9787", "iPod4-BlackRed"),  // 4G U2
      ("9436", "iPod3-Blue"),  // mini 1G blue
      ("9437", "iPod3-Gold"),  // mini 1G gold (2G dropped it)
      ("9803", "iPod3B-Blue"),  // mini 2G brighter blue
      ("9829", "iPod4-White"),  // photo
      ("A002", "iPod5-White"),  // Video 5G white
      ("A146", "iPod6-Black"),  // Video 5G black (no iPod5-Black asset)
      ("A452", "iPod5-BlackRed"),  // Video 5G U2
      ("A448", "iPod6-White"),  // Video 5.5G 80GB
      ("A107", "iPod7-Black"),  // nano 1G black
      ("A489", "iPod9-Pink"),  // nano 2G pink
      ("B249", "iPod12-Blue"),  // nano 3G blue
      ("B917", "iPod15-Red"),  // nano 4G red 16GB
      ("C043", "iPod16-Yellow"),  // nano 5G yellow
      ("C694", "iPod17-DarkGray"),  // nano 6G black → graphite art
      ("B029", "iPod11-Silver"),  // Classic 2007 80 silver
      ("B565", "iPod11-Black"),  // Classic 2008 120 black
      ("C293", "iPod11-Silver"),  // Classic 2009 silver
      ("C297", "iPod11B-Black"),  // Classic 2009 black (blacker art)
    ]
    for (model, icon) in expectations {
      XCTAssertEqual(DeviceIconLogic.iconBaseName(modelNum: model), icon, "model \(model)")
    }
  }

  func testUnknownModelAndShufflesFallBackToSilverClassic() {
    XCTAssertEqual(DeviceIconLogic.iconBaseName(modelNum: nil), "iPod11-Silver")
    XCTAssertEqual(DeviceIconLogic.iconBaseName(modelNum: "ZZZZ"), "iPod11-Silver")
    // Shuffles are deliberately unmapped (no iTunesDB, unsyncable).
    XCTAssertEqual(DeviceIconLogic.iconBaseName(modelNum: "C584"), "iPod11-Silver")
  }

  func testCacheKeyRetainsDeviceIdentityAcrossDisconnectedRows() {
    XCTAssertNotEqual(
      DeviceIconLogic.cacheKey(serial: "A", drive: nil),
      DeviceIconLogic.cacheKey(serial: "B", drive: nil))
    XCTAssertEqual(
      DeviceIconLogic.cacheKey(serial: "A", drive: "/Volumes/iPod"),
      DeviceIconLogic.cacheKey(serial: "A", drive: "/Volumes/iPod"))
  }

  func testCacheKeyUsesNonIdentityFallbackWhenDeviceIDIsUnavailable() {
    XCTAssertEqual(
      DeviceIconLogic.cacheKey(serial: nil, drive: nil),
      "<unknown>|<disconnected>")
  }

  /// Every icon the table can name must exist in the AMPDevices resources
  /// on this Mac — catches both table typos and an OS release moving the
  /// framework (skips rather than fails when the dir is gone entirely,
  /// since the view falls back gracefully at runtime).
  func testAllTableIconsExistOnThisSystem() throws {
    try XCTSkipUnless(
      FileManager.default.fileExists(atPath: DeviceIconLogic.ampResourcesDir),
      "AMPDevices resources not present on this OS — runtime falls back to volume icon/SF Symbol")
    for name in DeviceIconLogic.allIconBaseNames.sorted() {
      XCTAssertTrue(
        FileManager.default.fileExists(atPath: "\(DeviceIconLogic.ampResourcesDir)/\(name).icns"),
        "missing icon resource: \(name).icns")
    }
  }
}

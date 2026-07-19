import UniformTypeIdentifiers
import XCTest
@testable import Classick

final class LibraryDragPayloadTests: XCTestCase {
  private let nonce = UUID(uuidString: "00000000-0000-0000-0000-000000000001")!

  func testPayloadRejectsAnotherLaunch() throws {
    let payload = LibraryDragPayload(
      version: 1, launchNonce: nonce,
      rules: [.artist(name: "Birdy")], summary: "Birdy")

    XCTAssertThrowsError(try payload.validated(expectedNonce: UUID()))
  }

  @MainActor
  func testAppModelOwnsOneStableLaunchNonce() {
    let model = AppModel()
    let first = model.libraryDragLaunchNonce
    XCTAssertEqual(model.libraryDragLaunchNonce, first)
    XCTAssertNotEqual(AppModel().libraryDragLaunchNonce, first)
  }

  func testPayloadRoundTripContainsNoPathsOrTarget() throws {
    let payload = LibraryDragPayload(
      version: 1, launchNonce: nonce,
      rules: [.album(artist: "Birdy", album: "Fire Within")], summary: "Fire Within")

    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    let data = try encoder.encode(payload)
    let json = String(decoding: data, as: UTF8.self)

    XCTAssertEqual(
      json,
      #"{"launchNonce":"00000000-0000-0000-0000-000000000001","rules":[{"album":"Fire Within","artist":"Birdy","kind":"album"}],"summary":"Fire Within","version":1}"#)
    XCTAssertFalse(json.contains("/Volumes"))
    XCTAssertFalse(json.contains("serial"))
    XCTAssertFalse(json.contains("slug"))
    XCTAssertEqual(try JSONDecoder().decode(LibraryDragPayload.self, from: data), payload)
  }

  func testExportsClassickLibrarySelectionUTType() {
    XCTAssertEqual(
      UTType.classickLibrarySelection.identifier,
      "st.michaelwe.classick.library-selection")
  }

  func testValidationRejectsWrongVersionAndRuleCountBounds() {
    XCTAssertThrowsError(try payload(version: 2).validated(expectedNonce: nonce))
    XCTAssertThrowsError(try payload(rules: []).validated(expectedNonce: nonce))
    XCTAssertThrowsError(try payload(rules: Array(repeating: .artist(name: "Birdy"), count: 65)).validated(expectedNonce: nonce))
  }

  func testValidationRejectsBlankOversizedDuplicateAndNonNormalizedRules() {
    XCTAssertThrowsError(try payload(rules: [.artist(name: " ")]).validated(expectedNonce: nonce))
    XCTAssertThrowsError(try payload(rules: [.genre(name: String(repeating: "x", count: 257))]).validated(expectedNonce: nonce))
    XCTAssertThrowsError(try payload(rules: [.album(artist: "Birdy", album: "")]).validated(expectedNonce: nonce))
    XCTAssertThrowsError(try payload(rules: [.artist(name: " Birdy")]).validated(expectedNonce: nonce))
    XCTAssertThrowsError(try payload(rules: [.artist(name: "Birdy"), .artist(name: "birdy")]).validated(expectedNonce: nonce))
  }

  func testValidationRejectsInvalidSummary() {
    XCTAssertThrowsError(try payload(summary: "").validated(expectedNonce: nonce))
    XCTAssertThrowsError(try payload(summary: " Birdy").validated(expectedNonce: nonce))
    XCTAssertThrowsError(try payload(summary: String(repeating: "x", count: 129)).validated(expectedNonce: nonce))
  }

  func testMakeNormalizesRuleAndSummary() throws {
    let made = try LibraryDragPayload.make(
      rule: .album(artist: " Birdy ", album: " Fire Within "),
      summary: " Fire Within ", launchNonce: nonce)

    XCTAssertEqual(made.rules, [.album(artist: "Birdy", album: "Fire Within")])
    XCTAssertEqual(made.summary, "Fire Within")
    XCTAssertEqual(try made.validated(expectedNonce: nonce), made.rules)
  }

  private func payload(
    version: UInt16 = 1,
    rules: [SelectionRule] = [.artist(name: "Birdy")],
    summary: String = "Birdy"
  ) -> LibraryDragPayload {
    LibraryDragPayload(
      version: version, launchNonce: nonce, rules: rules, summary: summary)
  }
}

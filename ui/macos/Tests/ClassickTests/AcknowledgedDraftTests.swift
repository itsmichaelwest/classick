import XCTest
@testable import Classick

final class AcknowledgedDraftTests: XCTestCase {
  private struct GlobalSettings: Equatable {
    var source: String
    var schedule: UInt32
  }

  func testProgrammaticSeedIsCleanAndDoesNotCreateSubmission() {
    var draft = AcknowledgedDraft(canonical: "A", revision: 1)

    draft.reconcile(canonical: "B", revision: 2, acknowledgedRequestID: nil)

    XCTAssertEqual(draft.value, "B")
    XCTAssertEqual(draft.canonicalRevision, 2)
    XCTAssertFalse(draft.isDirty)
    XCTAssertTrue(draft.submitted.isEmpty)
  }

  func testOlderAcknowledgementCannotCleanOrReplaceNewerEdit() {
    var draft = AcknowledgedDraft(canonical: "seed", revision: 0)
    draft.edit("A")
    draft.markSubmitted(requestID: "request-a")
    draft.edit("B")
    draft.markSubmitted(requestID: "request-b")

    draft.reconcile(canonical: "A", revision: 1, acknowledgedRequestID: "request-a")

    XCTAssertEqual(draft.value, "B")
    XCTAssertTrue(draft.isDirty)
    XCTAssertNil(draft.submitted["request-a"])
    XCTAssertNotNil(draft.submitted["request-b"])

    draft.reconcile(canonical: "B", revision: 2, acknowledgedRequestID: "request-b")

    XCTAssertEqual(draft.value, "B")
    XCTAssertFalse(draft.isDirty)
    XCTAssertTrue(draft.submitted.isEmpty)
  }

  func testStaleRevisionCannotRollBackCanonicalOrVisibleValue() {
    var draft = AcknowledgedDraft(canonical: "new", revision: 8)

    draft.reconcile(canonical: "old", revision: 7, acknowledgedRequestID: nil)

    XCTAssertEqual(draft.value, "new")
    XCTAssertEqual(draft.canonicalRevision, 8)
    XCTAssertFalse(draft.isDirty)
  }

  func testUnsolicitedCanonicalUpdatePreservesPendingLocalEdit() {
    var draft = AcknowledgedDraft(canonical: "seed", revision: 1)
    draft.edit("local")

    draft.reconcile(canonical: "external", revision: 2, acknowledgedRequestID: nil)

    XCTAssertEqual(draft.value, "local")
    XCTAssertEqual(draft.canonicalRevision, 2)
    XCTAssertTrue(draft.isDirty)
  }

  func testEditingBackToCanonicalBecomesClean() {
    var draft = AcknowledgedDraft(canonical: "seed", revision: 1)
    draft.edit("changed")
    draft.edit("seed")

    XCTAssertFalse(draft.isDirty)
  }

  func testGlobalSettingsBroadcastPreservesPendingLocalEdit() {
    var draft = AcknowledgedDraft(
      canonical: GlobalSettings(source: "/music", schedule: 60), revision: 1)
    draft.edit(GlobalSettings(source: "/music", schedule: 180))
    draft.markSubmitted(requestID: "local-save")

    draft.reconcile(
      canonical: GlobalSettings(source: "/other", schedule: 30), revision: 2,
      acknowledgedRequestID: "another-client")

    XCTAssertEqual(draft.value, GlobalSettings(source: "/music", schedule: 180))
    XCTAssertTrue(draft.isDirty)
    XCTAssertNotNil(draft.submitted["local-save"])
  }

  func testExternalDropAdvancesCanonicalWithoutErasingDirtyDeviceEdit() {
    var draft = AcknowledgedDraft(canonical: "original", revision: 4)
    draft.edit("local")

    draft.reconcile(canonical: "dropped", revision: 5, acknowledgedRequestID: "drop")

    XCTAssertEqual(draft.value, "local")
    XCTAssertEqual(draft.canonicalRevision, 5)
    XCTAssertTrue(draft.isDirty)
  }

  func testDropAckBeforeLaterEditorAckPreservesGenerationTruth() {
    var draft = AcknowledgedDraft(canonical: "original", revision: 4)
    draft.edit("local")
    draft.markSubmitted(requestID: "save-b")

    draft.reconcile(canonical: "dropped", revision: 5, acknowledgedRequestID: "drop-a")
    XCTAssertEqual(draft.value, "local")
    XCTAssertTrue(draft.isDirty)

    draft.reconcile(canonical: "local", revision: 6, acknowledgedRequestID: "save-b")
    XCTAssertEqual(draft.value, "local")
    XCTAssertFalse(draft.isDirty)
  }

  func testAcknowledgementRequiresExactRequestAndMutation() {
    var draft = AcknowledgedDraft(canonical: "original", revision: 1)
    draft.edit("local")
    draft.markSubmitted(requestID: "request", mutationID: "mutation")

    draft.reconcile(
      canonical: "other", revision: 2, acknowledgedRequestID: "request",
      acknowledgedMutationID: "different")

    XCTAssertEqual(draft.value, "local")
    XCTAssertTrue(draft.isDirty)
    XCTAssertTrue(draft.hasPendingSubmission)
  }

  func testHostRejectionRetainsAttemptAsDirtyDraft() {
    var draft = AcknowledgedDraft(canonical: "original", revision: 1)
    draft.edit("local")
    draft.markSubmitted(requestID: "request", mutationID: "mutation")

    XCTAssertTrue(
      draft.reject(requestID: "request", mutationID: "mutation", message: "Disk full"))

    XCTAssertEqual(draft.value, "local")
    XCTAssertTrue(draft.isDirty)
    XCTAssertFalse(draft.hasPendingSubmission)
    XCTAssertEqual(draft.hostAcceptanceFailure, "Disk full")
  }
}

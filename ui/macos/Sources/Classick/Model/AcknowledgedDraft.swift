struct SubmittedDraft<Value: Equatable>: Equatable {
  let requestID: String
  let mutationID: String?
  let generation: UInt64
  let value: Value
}

/// A locally editable value reconciled against monotonically revised daemon state.
///
/// The daemon remains canonical, but an acknowledgement for an older generation
/// must never replace or clean a newer local edit.
struct AcknowledgedDraft<Value: Equatable>: Equatable {
  private(set) var value: Value
  private(set) var canonicalRevision: UInt64
  private(set) var submitted: [String: SubmittedDraft<Value>] = [:]
  private(set) var isDirty = false
  private(set) var hostAcceptanceFailure: String?
  private(set) var acceptedRequestID: String?
  private(set) var acceptedMutationID: String?

  private var canonicalValue: Value
  private var generation: UInt64 = 0

  init(canonical: Value, revision: UInt64) {
    value = canonical
    canonicalValue = canonical
    canonicalRevision = revision
  }

  mutating func edit(_ value: Value) {
    guard value != self.value else { return }
    generation &+= 1
    self.value = value
    isDirty = value != canonicalValue
    hostAcceptanceFailure = nil
  }

  mutating func markSubmitted(requestID: String, mutationID: String? = nil) {
    submitted[requestID] = SubmittedDraft(
      requestID: requestID, mutationID: mutationID, generation: generation, value: value)
    hostAcceptanceFailure = nil
  }

  var hasPendingSubmission: Bool { !submitted.isEmpty }

  var hasUnsubmittedChanges: Bool {
    isDirty && !submitted.values.contains(where: { $0.generation == generation })
  }

  mutating func reconcile(
    canonical: Value, revision: UInt64, acknowledgedRequestID: String?,
    acknowledgedMutationID: String? = nil
  ) {
    guard revision >= canonicalRevision else { return }
    canonicalRevision = revision
    canonicalValue = canonical
    if acknowledgedMutationID != acceptedMutationID {
      acceptedRequestID = nil
      acceptedMutationID = acknowledgedMutationID
    }

    guard let acknowledgedRequestID,
      let acknowledgement = submitted[acknowledgedRequestID],
      acknowledgement.mutationID == nil || acknowledgement.mutationID == acknowledgedMutationID
    else {
      if !isDirty { value = canonical }
      isDirty = value != canonicalValue
      return
    }

    submitted = submitted.filter { $0.value.generation > acknowledgement.generation }
    hostAcceptanceFailure = nil
    acceptedRequestID = acknowledgedRequestID
    acceptedMutationID = acknowledgedMutationID
    if generation <= acknowledgement.generation {
      value = canonical
    }
    isDirty = value != canonicalValue
  }

  @discardableResult
  mutating func reject(
    requestID: String, mutationID: String?, message: String
  ) -> Bool {
    guard let submission = submitted[requestID],
      submission.mutationID == nil || submission.mutationID == mutationID
    else { return false }
    submitted.removeValue(forKey: requestID)
    if generation <= submission.generation {
      value = submission.value
      isDirty = value != canonicalValue
      hostAcceptanceFailure = message
    }
    return true
  }

  func acceptsDeliveryFailure(requestID: String, mutationID: String) -> Bool {
    acceptedRequestID == requestID && acceptedMutationID == mutationID
  }

  mutating func prepareForProtocolReconnect() {
    guard !submitted.isEmpty else { return }
    submitted.removeAll()
    isDirty = true
    hostAcceptanceFailure = nil
  }
}

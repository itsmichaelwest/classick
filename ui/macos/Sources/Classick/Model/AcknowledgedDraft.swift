struct SubmittedDraft<Value: Equatable>: Equatable {
  let requestID: String
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
  }

  mutating func markSubmitted(requestID: String) {
    submitted[requestID] = SubmittedDraft(
      requestID: requestID, generation: generation, value: value)
  }

  mutating func reconcile(
    canonical: Value, revision: UInt64, acknowledgedRequestID: String?
  ) {
    guard revision >= canonicalRevision else { return }
    canonicalRevision = revision
    canonicalValue = canonical

    guard let acknowledgedRequestID,
      let acknowledgement = submitted[acknowledgedRequestID]
    else {
      if !isDirty { value = canonical }
      isDirty = value != canonicalValue
      return
    }

    submitted = submitted.filter { $0.value.generation > acknowledgement.generation }
    if generation <= acknowledgement.generation {
      value = canonical
    }
    isDirty = value != canonicalValue
  }
}

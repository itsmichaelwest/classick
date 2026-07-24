import SwiftUI

/// The app's motion vocabulary. Apple's fluid-interface guidance: default UI
/// motion is critically damped — overshoot is earned by a gesture that
/// carried momentum into the animation, and nothing here is gesture-driven
/// (every value change arrives from the daemon). `.smooth` IS SwiftUI's
/// critically damped spring, so these are bounce-free by construction.
enum Motion {
  /// Progress/capacity fills and other geometry that tracks a live value.
  static let meter: Animation = .smooth(duration: 0.4)
  /// Chrome that appears, disappears, or swaps (meter, action buttons).
  static let chrome: Animation = .smooth(duration: 0.3)
}

extension View {
  /// Animates `value` changes with `animation`, honoring Reduce Motion: with
  /// it on, the change lands directly instead of springing. Opacity
  /// transitions still read as an instant swap rather than a jump, which is
  /// the non-vestibular equivalent the setting asks for.
  func motion<V: Equatable>(_ animation: Animation, value: V) -> some View {
    modifier(ReduceMotionAware(animation: animation, value: value))
  }
}

private struct ReduceMotionAware<V: Equatable>: ViewModifier {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  var animation: Animation
  var value: V

  func body(content: Content) -> some View {
    content.animation(reduceMotion ? nil : animation, value: value)
  }
}

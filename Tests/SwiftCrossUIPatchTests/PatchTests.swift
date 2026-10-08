import Testing

/// The tests of the vendored SwiftCrossUI's patches (Vendor/PATCHES.md). One serialized suite:
/// they share SwiftCrossUI's global update queue (P8) and the fake backend's main loop.
@Suite(.serialized)
enum PatchTests {}

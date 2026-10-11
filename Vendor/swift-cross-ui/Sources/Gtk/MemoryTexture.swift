import CGtk

public struct MemoryTexture {
    public let pointer: OpaquePointer

    public init(rgbaData: [UInt8], width: Int, height: Int, format: Int, stride: Int) {
        // dashwallet-desktop patch P15: gsize imports as UInt64 on Windows.
        let bytes = g_bytes_new(rgbaData, gsize(rgbaData.count))
        pointer = gdk_memory_texture_new(Int32(width), Int32(height), GDK_MEMORY_R8G8B8A8, bytes, 4)
    }
}

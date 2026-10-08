// Files the user exports outside the data root (CSV of transactions or the
// address book): owner-only from creation, whatever the umask (review D1-r2).
import Foundation
import PlatformServices

public enum ExportFile {
    /// Writes `text` to `url` as a new 0600 file, replacing what the user
    /// chose to overwrite (`Data.write(options: .atomic)`'s behaviour).
    public static func write(_ text: String, to url: URL) throws {
        try PrivateFileSystem.writeFile(Data(text.utf8), to: url, replacing: true)
    }
}

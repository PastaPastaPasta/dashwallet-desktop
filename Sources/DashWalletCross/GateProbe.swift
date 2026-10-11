// Opt-in marks and harness hooks for the UI-stack gate (G-03, docs/adr/0003-ui-stack.md). Off
// unless DWD_GATE=1. Each mark is one stderr line `gate-mark NAME SECONDS [key=value...] cpu=S`,
// where `cpu` is the process's CPU time when the line is written.
//
// - Linux: SECONDS is CLOCK_MONOTONIC, the clock of the harness's `time.monotonic()`, so a mark
//   lines up with the time the harness started the process (U5). The screen marks come from the
//   window's GdkFrameClock "after-paint" signal: the end of the first frame GTK drew once the
//   screen's condition held.
// - macOS and Windows (no frame hook): the same marks from a 250 ms poll of the view models, so
//   the CI harness knows when to take a screenshot. SECONDS is the system uptime.
//
// Harness hooks, for runners without AT-SPI: DWD_GATE_UNLOCK_FILE=<path> unlocks the vault with
// the file's first line through the lock screen's view model, DWD_GATE_HIDE_SYNC_OVERLAY=1 hides
// the sync overlay (as its Hide button does; the gate wallet has no peer, so it stays behind), and
// DWD_GATE_OPEN_DETAIL=1 opens the first row's details on Transactions (as selecting it does).
import CrossUI
import Foundation
import SwiftCrossUI
import WalletFeatures
import WalletRuntime

#if os(Linux)
    import CGtk
    import Gtk
    import GtkBackend
#endif

@MainActor
enum GateProbe {
    nonisolated static let enabled = ProcessInfo.processInfo.environment["DWD_GATE"] == "1"
    /// `DWD_GATE_HISTORY_BENCH=N`: once Transactions shows rows, N rounds of `history_page`
    /// with limit 200 (the FFI call) followed by drawing those 200 rows, then N with limit 1.
    nonisolated static let historyRounds = Int(ProcessInfo.processInfo.environment["DWD_GATE_HISTORY_BENCH"] ?? "") ?? 0
    private static let unlockFile = ProcessInfo.processInfo.environment["DWD_GATE_UNLOCK_FILE"]
    private static let openDetail = ProcessInfo.processInfo.environment["DWD_GATE_OPEN_DETAIL"] == "1"
    private static let hideSyncOverlay = ProcessInfo.processInfo.environment["DWD_GATE_HIDE_SYNC_OVERLAY"] == "1"

    nonisolated static func now() -> Double {
        #if os(Linux)
            var time = timespec()
            clock_gettime(CLOCK_MONOTONIC, &time)
            return Double(time.tv_sec) + Double(time.tv_nsec) / 1e9
        #else
            return ProcessInfo.processInfo.systemUptime
        #endif
    }

    /// The process's CPU time (all threads), in seconds.
    nonisolated static func cpu() -> Double {
        var time = timespec()
        clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &time)
        return Double(time.tv_sec) + Double(time.tv_nsec) / 1e9
    }

    /// Writes the mark with the process's CPU time when it is written (`cpu=`).
    nonisolated static func mark(_ name: String, at time: Double? = nil, _ fields: String = "") {
        guard enabled else { return }
        let all = (fields.isEmpty ? "" : "\(fields) ") + "cpu=\(String(format: "%.3f", cpu()))"
        let line = "gate-mark \(name) \(String(format: "%.6f", time ?? now())) \(all)\n"
        FileHandle.standardError.write(Data(line.utf8))
    }

    private static var host: CrossAppHost?
    private static var firstPainted = false
    /// The screen the last mark named; a mark is written each time it changes.
    private static var shown: String?
    private static var paintWaiters: [Int: CheckedContinuation<Double?, Never>] = [:]
    private static var nextWaiter = 0
    private static var unlockSent = false
    private static var detailOpened = false
    private static var benchState = BenchState.idle

    private enum BenchState { case idle, running, done }

    /// Called after every frame GTK draws (Linux), or every 250 ms (elsewhere). Writes
    /// `first-paint` once, then `<screen>-paint` whenever the screen changes: `lock`,
    /// `sync-overlay`, `overview` (once it lists transactions), `transactions` (with rows) and
    /// `detail`. A live launch can
    /// show Overview before the vault reports it is locked, so a harness waiting for Overview
    /// after an unlock takes the first `overview-paint` after `unlock-requested`.
    static func check(at time: Double, painted: Bool) {
        if painted {
            for (_, waiter) in paintWaiters { waiter.resume(returning: time) }
            paintWaiters = [:]
            if !firstPainted {
                firstPainted = true
                mark("first-paint", at: time)
            }
        }
        guard let state = host?.state, benchState != .running else { return }
        let main = state.main
        let model = main.selection == .transactions ? state.transactions() : nil
        let screen: String? =
            if main.showsLockScreen {
                "lock"
            } else if main.showsSyncOverlay {
                "sync-overlay"
            } else if main.selection == .overview, !(main.home?.recent.isEmpty ?? true) {
                "overview"
            } else if let model, !model.rows.isEmpty {
                model.detail == nil ? "transactions" : "detail"
            } else {
                nil
            }
        if screen != shown {
            shown = screen
            if let screen {
                let rows = screen == "transactions" ? "rows=\(model?.rows.count ?? 0)" : ""
                mark("\(screen)-paint", at: time, rows)
            }
        }
        if screen == "lock", let unlockFile, !unlockSent,
            let passphrase = try? String(contentsOfFile: unlockFile, encoding: .utf8)
                .split(separator: "\n", omittingEmptySubsequences: false).first
        {
            unlockSent = true
            mark("unlock-requested")
            Task { await main.lock.unlock(passphrase: String(passphrase), mixingOnly: false) }
        }
        if screen == "sync-overlay", hideSyncOverlay {
            mark("sync-overlay-hide")
            main.hideSyncOverlay()
        }
        guard screen == "transactions", let model else { return }
        if openDetail, !detailOpened, let first = model.rows.first {
            detailOpened = true
            Task { await model.select(first.id) }
        }
        // A live launch can list rows before the vault reports it is locked; the bench waits
        // for its own unlock.
        if historyRounds > 0, benchState == .idle, unlockFile == nil || unlockSent {
            benchState = .running
            Task {
                await historyBench(state: state, model: model)
                benchState = .done
            }
        }
    }

    /// Without a frame hook: checks every 250 ms for as long as the app runs.
    static func poll(_ host: CrossAppHost) async {
        guard enabled, self.host == nil else { return }
        self.host = host
        while !Task.isCancelled {
            check(at: now(), painted: false)
            try? await Task.sleep(for: .milliseconds(250))
        }
    }

    /// The end of the next frame, or `nil` if GTK draws none within `timeout` seconds.
    static func nextPaint(timeout: Double = 5) async -> Double? {
        nextWaiter += 1
        let id = nextWaiter
        return await withCheckedContinuation { continuation in
            paintWaiters[id] = continuation
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(timeout))
                paintWaiters.removeValue(forKey: id)?.resume(returning: nil)
            }
        }
    }

    /// U5 "history_page(200) round trip (SwiftCrossUI: FFI call + render)". Each round empties
    /// the page first (and waits for that frame), so every round draws 200 new rows.
    private static func historyBench(state: CrossAppState, model: TransactionsViewModel) async {
        guard let wallet = state.env.walletState.selectedWalletID else {
            mark("history-bench-error", "reason=no-wallet")
            return
        }
        for limit in [200, 1] {
            for round in 0..<historyRounds {
                model.replaceRowsForMeasurement([])
                _ = await nextPaint()
                let start = now()
                let startCPU = cpu()
                guard let page = try? await state.env.history.page(wallet: wallet, query: HistoryQuery(limit: limit)) else {
                    mark("history-bench-error", "reason=page limit=\(limit)")
                    return
                }
                let called = now()
                guard limit > 1 else {
                    let fields = "limit=1 round=\(round) ffiMs=\(ms(called - start)) cpuMs=\(ms(cpu() - startCPU))"
                    mark("history-ffi", at: called, fields)
                    continue
                }
                model.replaceRowsForMeasurement(page.records)
                let drawn = await nextPaint()
                let fields = "limit=\(limit) round=\(round) rows=\(page.records.count) ffiMs=\(ms(called - start))"
                if let drawn {
                    let times = "renderMs=\(ms(drawn - called)) totalMs=\(ms(drawn - start)) cpuMs=\(ms(cpu() - startCPU))"
                    mark("history-render", at: drawn, "\(fields) \(times)")
                } else {
                    mark("history-render", "\(fields) renderMs=timeout")
                }
            }
        }
        mark("history-bench-done")
    }

    private static func ms(_ seconds: Double) -> String { String(format: "%.3f", seconds * 1000) }

    #if os(Linux)
        private static var clockConnected = false

        /// Connects `check` to the window's frame clock (once the window is realized).
        static func attach(to window: Gtk.ApplicationWindow, host: CrossAppHost) {
            guard enabled, !clockConnected else { return }
            self.host = host
            let widget = UnsafeMutableRawPointer(window.widgetPointer).assumingMemoryBound(to: GtkWidget.self)
            if gtk_widget_get_realized(widget) != 0 {
                connectFrameClock(of: widget)
            } else {
                let realized: @convention(c) (UnsafeMutablePointer<GtkWidget>?, UnsafeMutableRawPointer?) -> Void = {
                    widget, _ in
                    guard let widget else { return }
                    MainActor.assumeIsolated { GateProbe.connectFrameClock(of: widget) }
                }
                g_signal_connect_data(
                    widget, "realize", unsafeBitCast(realized, to: GCallback.self), nil, nil,
                    GConnectFlags(rawValue: 0))
            }
        }

        private static func connectFrameClock(of widget: UnsafeMutablePointer<GtkWidget>) {
            guard !clockConnected, let clock = gtk_widget_get_frame_clock(widget) else { return }
            clockConnected = true
            let afterPaint: @convention(c) (UnsafeMutableRawPointer?, UnsafeMutableRawPointer?) -> Void = { _, _ in
                MainActor.assumeIsolated { GateProbe.check(at: GateProbe.now(), painted: true) }
            }
            g_signal_connect_data(
                UnsafeMutableRawPointer(clock), "after-paint", unsafeBitCast(afterPaint, to: GCallback.self), nil, nil,
                GConnectFlags(rawValue: 0))
        }
    #endif
}

extension View {
    /// The gate's marks and hooks (`DWD_GATE=1`): the frame hook on Linux, a poll elsewhere.
    @ViewBuilder
    func gateProbe(_ host: CrossAppHost) -> some View {
        if GateProbe.enabled {
            #if os(Linux)
                inspectWindow { window in
                    GateProbe.attach(to: window, host: host)
                }
            #else
                task { await GateProbe.poll(host) }
            #endif
        } else {
            self
        }
    }
}

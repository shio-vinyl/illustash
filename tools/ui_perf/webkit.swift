// 界面性能测量（#683）的 WebKit 驱动：一个只开网页的小程序。用系统的 WKWebView（和桌面 App 同一个内核）打开隔离宿主
// 的页面，注入 probe.js，按计划逐步执行；内存检查点读 WebContent 进程（和能拿到时的 WebKit GPU 进程）的 footprint。
// 不启动 App，不读 App 的设置和钥匙串；网页数据用不落盘的 nonPersistent 存储（用户 2026-11-19 定，#703 第 4 问）。
//
// 窗口要真的在屏幕上：被完全挡住或不在屏幕上时 WebKit 会停掉 requestAnimationFrame 和绘制。窗口不抢焦点（accessory
// 应用，不进程序坞），尺寸和 Chromium 一侧相同；能用时关掉 WebKit 的「窗口被挡住就节流」（私有接口，拿不到就照常）。
// 计时期间不要把它挡住。
//
// 编译和用法（由 tools/ui_perf/__main__.py 调用）：swiftc -O webkit.swift -o perf-webkit；perf-webkit <参数 JSON 文件>，
// 参数是 {url, probe, plan, width, height, out, root}。结果 JSON 写到 out。
import AppKit
import Foundation
import WebKit

struct Args: Decodable {
    let url: String
    let probe: String
    let plan: [Step]
    let width: Double
    let height: Double
    let out: String
    let root: String
}

struct Step: Decodable {
    let op: String
    let label: String?
    let args: [String: AnyDecodable]?
}

struct AnyDecodable: Decodable {
    let value: Any
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if let v = try? c.decode(Bool.self) { value = v } else if let v = try? c.decode(Double.self) { value = v }
        else if let v = try? c.decode(String.self) { value = v } else if let v = try? c.decode([String: AnyDecodable].self) {
            value = v.mapValues { $0.value }
        } else if let v = try? c.decode([AnyDecodable].self) { value = v.map { $0.value } } else { value = NSNull() }
    }
}

final class Driver: NSObject, WKNavigationDelegate {
    let args: Args
    let probe: String
    let window: NSWindow
    let view: WKWebView
    var loaded: CheckedContinuation<Void, Never>?

    init(args: Args) throws {
        self.args = args
        probe = try String(contentsOfFile: args.probe, encoding: .utf8)
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        let frame = NSRect(x: 0, y: 0, width: args.width, height: args.height)
        view = WKWebView(frame: frame, configuration: config)
        let screen = NSScreen.main?.visibleFrame ?? frame
        window = NSWindow(contentRect: NSRect(x: screen.minX, y: screen.maxY - args.height, width: args.width, height: args.height),
                          styleMask: [.titled], backing: .buffered, defer: false)
        window.title = "Illustash 性能测量（合成图库）"
        window.contentView = view
        super.init()
        view.navigationDelegate = self
        let occlusion = NSSelectorFromString("_setWindowOcclusionDetectionEnabled:")
        if view.responds(to: occlusion) { view.perform(occlusion, with: false) }
        window.orderFrontRegardless()
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        loaded?.resume()
        loaded = nil
    }

    func call(_ op: String, _ a: [String: Any]) async throws -> Any {
        let data = try JSONSerialization.data(withJSONObject: a)
        let js = probe + "\n;return await window.__perf.call(\(jsonString(op)), \(String(data: data, encoding: .utf8)!));"
        let r = try await view.callAsyncJavaScript(js, arguments: [:], in: nil, contentWorld: .page)
        guard let dict = r as? [String: Any] else { throw Failure("\(op)：探针没有回应") }
        if (dict["ok"] as? Bool) != true { throw Failure("\(op)：\(dict["error"] ?? "")") }
        return dict["value"] ?? NSNull()
    }

    func pid(_ key: String) -> Int? {
        let selector = NSSelectorFromString(key)
        guard view.responds(to: selector) else { return nil }
        return (view.value(forKey: key) as? NSNumber)?.intValue
    }

    func footprint(_ pids: [Int]) throws -> [String: Any] {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        p.arguments = ["python3", "-m", "tools.ui_perf.footprint"] + pids.map(String.init)
        p.currentDirectoryURL = URL(fileURLWithPath: args.root)
        let pipe = Pipe()
        p.standardOutput = pipe
        try p.run()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return (try JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
    }

    func run() async -> [String: Any] {
        var result: [String: Any] = ["engine": "webkit"]
        let webkit = Bundle(for: WKWebView.self)
        result["browser"] = "WebKit \(webkit.infoDictionary?["CFBundleVersion"] ?? "?") (macOS \(ProcessInfo.processInfo.operatingSystemVersionString))"
        result["backingScale"] = window.backingScaleFactor
        var steps: [[String: Any]] = []
        do {
            await withCheckedContinuation { c in
                loaded = c
                view.load(URLRequest(url: URL(string: args.url)!))
            }
            // 一次性码地址换完 cookie 后页面会再跳一次，跳转中的调用拿不到回应：重试到应用启动完（只防挂死，60 秒）。
            var boot: Any?
            let deadline = Date().addingTimeInterval(60)
            while boot == nil {
                do { boot = try await call("boot", [:]) } catch {
                    if Date() > deadline { throw error }
                    try await Task.sleep(nanoseconds: 300_000_000)
                }
            }
            result["boot"] = boot
            for step in args.plan {
                let t0 = Date()
                let a = step.args?.mapValues { $0.value } ?? [:]
                var value: Any = [String: Any]()
                switch step.op {
                case "traceStart", "traceStop":
                    break    // WebKit 没有对应的性能记录接口，只有探针自己的帧和 DOM 指标
                case "memory":
                    if let settle = a["settle"] as? Double { try await Task.sleep(nanoseconds: UInt64(settle * 1_000_000)) }
                    var out: [String: Any] = [:]
                    var wanted: [(String, Int)] = []
                    if let w = pid("_webProcessIdentifier") { wanted.append(("webcontent", w)) }
                    if let g = pid("_gpuProcessIdentifier") { wanted.append(("gpu", g)) }
                    let fp = try footprint(wanted.map { $0.1 })
                    for (k, p) in wanted { out[k] = fp[String(p)] }
                    out["dom"] = try await call("snapshot", a)
                    value = out
                default:
                    value = try await call(step.op, a)
                }
                steps.append(["op": step.op, "label": step.label ?? "", "seconds": (Date().timeIntervalSince(t0) * 10).rounded() / 10,
                              "value": value])
                // 每步写一次，卡住被杀掉时也留下已经做完的步骤。
                result["steps"] = steps
                result["error"] = "未完成：停在第 \(steps.count + 1) 步之前"
                write(result)
                result["error"] = nil
            }
        } catch {
            result["error"] = "\(error)"
        }
        result["steps"] = steps
        return result
    }

    func write(_ result: [String: Any]) {
        if let data = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) {
            try? data.write(to: URL(fileURLWithPath: args.out))
        }
    }
}

struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ d: String) { description = d }
}

func jsonString(_ s: String) -> String {
    String(data: try! JSONSerialization.data(withJSONObject: [s]), encoding: .utf8)!.dropFirst().dropLast().description
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let argsPath = CommandLine.arguments.dropFirst().first ?? ""
guard let raw = FileManager.default.contents(atPath: argsPath), let args = try? JSONDecoder().decode(Args.self, from: raw) else {
    FileHandle.standardError.write("用法：perf-webkit <参数 JSON 文件>\n".data(using: .utf8)!)
    exit(2)
}
Task { @MainActor in
    do {
        let driver = try Driver(args: args)
        let result = await driver.run()
        let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
        try data.write(to: URL(fileURLWithPath: args.out))
    } catch {
        FileHandle.standardError.write("\(error)\n".data(using: .utf8)!)
        exit(1)
    }
    exit(0)
}
app.run()

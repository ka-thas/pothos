// pothos publish helper: image rendering and R2 (S3 API) uploads, using only macOS frameworks.
//
// Usage:
//   publish-helper info <file>...                            -> JSON [{"takenAt": "..."}] in the same order
//   publish-helper render <src> <full.jpg> <thumb.jpg> <px>  -> JSON {"width": w, "height": h}
//   publish-helper get <key> <out-file>                      -> exit 0, or 3 if the key does not exist
//   publish-helper put <key> <file> <content-type> <cache-control>
//   publish-helper delete <key>
//   publish-helper selftest                                  -> checks the request signer against AWS's test vector
//
// Bucket access comes from the environment (never argv, so keys stay out of `ps`):
//   R2_ENDPOINT (https://<account>.r2.cloudflarestorage.com), R2_BUCKET, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY
import CryptoKit
import Foundation
import ImageIO
import UniformTypeIdentifiers

func fail(_ msg: String, _ code: Int32 = 1) -> Never {
    FileHandle.standardError.write((msg + "\n").data(using: .utf8)!)
    exit(code)
}

func printJSON(_ v: Any) {
    let d = try! JSONSerialization.data(withJSONObject: v, options: [.sortedKeys])
    FileHandle.standardOutput.write(d)
}

// MARK: - Capture time

/// ISO 8601 with the Mac's current time zone offset, like 2026-11-20T15:32:00+09:00.
func iso(_ d: Date) -> String {
    let f = DateFormatter()
    f.locale = Locale(identifier: "en_US_POSIX")
    f.timeZone = TimeZone.current
    f.dateFormat = "yyyy-MM-dd'T'HH:mm:ssXXXXX"
    return f.string(from: d)
}

/// EXIF DateTimeOriginal (with OffsetTimeOriginal if present), else the file's modified time.
/// Cameras like the Charmera write no EXIF, so the modified time is their capture time.
func takenAt(_ path: String) -> String {
    let url = URL(fileURLWithPath: path)
    if let src = CGImageSourceCreateWithURL(url as CFURL, nil),
       let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any],
       let exif = props[kCGImagePropertyExifDictionary] as? [CFString: Any],
       let dt = exif[kCGImagePropertyExifDateTimeOriginal] as? String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        if let off = exif["OffsetTimeOriginal" as CFString] as? String {
            f.dateFormat = "yyyy:MM:dd HH:mm:ssXXXXX"
            if let d = f.date(from: dt + off) {
                f.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
                f.timeZone = TimeZone(secondsFromGMT: 0)
                // Keep the camera's own offset rather than converting to the Mac's zone.
                let local = d.addingTimeInterval(Double(offsetSeconds(off)))
                return f.string(from: local) + off
            }
        }
        f.dateFormat = "yyyy:MM:dd HH:mm:ss"
        f.timeZone = TimeZone.current
        if let d = f.date(from: dt) { return iso(d) }
    }
    let attrs = try? FileManager.default.attributesOfItem(atPath: path)
    guard let m = attrs?[.modificationDate] as? Date else { fail("cannot read \(path)") }
    return iso(m)
}

func offsetSeconds(_ off: String) -> Int {
    let sign = off.hasPrefix("-") ? -1 : 1
    let parts = off.dropFirst().split(separator: ":").compactMap { Int($0) }
    guard parts.count == 2 else { return 0 }
    return sign * (parts[0] * 3600 + parts[1] * 60)
}

// MARK: - Rendering

/// Writes the image with its EXIF orientation applied to the pixels, at most `maxPx` on the long side.
/// Metadata (EXIF, GPS) is dropped. Returns the written pixel size.
func writeJPEG(_ src: CGImageSource, _ out: String, maxPx: Int, quality: Double) -> (Int, Int) {
    let opts: [CFString: Any] = [
        kCGImageSourceCreateThumbnailFromImageAlways: true,
        kCGImageSourceCreateThumbnailWithTransform: true,
        kCGImageSourceThumbnailMaxPixelSize: maxPx,
        kCGImageSourceShouldCacheImmediately: true,
    ]
    guard let img = CGImageSourceCreateThumbnailAtIndex(src, 0, opts as CFDictionary) else { fail("cannot decode image") }
    let url = URL(fileURLWithPath: out)
    guard let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.jpeg.identifier as CFString, 1, nil) else {
        fail("cannot write \(out)")
    }
    CGImageDestinationAddImage(dest, img, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
    if !CGImageDestinationFinalize(dest) { fail("cannot write \(out)") }
    return (img.width, img.height)
}

func render(_ path: String, _ full: String, _ thumb: String, _ thumbPx: Int) {
    guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any],
          let w = props[kCGImagePropertyPixelWidth] as? Int, let h = props[kCGImagePropertyPixelHeight] as? Int
    else { fail("cannot read \(path)") }
    let (fw, fh) = writeJPEG(src, full, maxPx: max(w, h), quality: 0.85)
    _ = writeJPEG(src, thumb, maxPx: thumbPx, quality: 0.8)
    printJSON(["width": fw, "height": fh])
}

// MARK: - AWS Signature Version 4

func hex(_ d: some Sequence<UInt8>) -> String { d.map { String(format: "%02x", $0) }.joined() }
func sha256(_ d: Data) -> String { hex(SHA256.hash(data: d)) }
func hmac(_ key: Data, _ msg: String) -> Data {
    Data(HMAC<SHA256>.authenticationCode(for: Data(msg.utf8), using: SymmetricKey(data: key)))
}

/// Returns the Authorization header for a request. `headers` must include host and x-amz-date
/// (lowercase names); every header passed is signed.
func authorization(method: String, path: String, query: String, headers: [String: String], payloadHash: String,
                   accessKey: String, secret: String, region: String, service: String) -> String {
    let names = headers.keys.sorted()
    let canonicalHeaders = names.map { "\($0):\(headers[$0]!.trimmingCharacters(in: .whitespaces))\n" }.joined()
    let signed = names.joined(separator: ";")
    let canonical = [method, path, query, canonicalHeaders, signed, payloadHash].joined(separator: "\n")
    let amzDate = headers["x-amz-date"]!
    let day = String(amzDate.prefix(8))
    let scope = "\(day)/\(region)/\(service)/aws4_request"
    let toSign = ["AWS4-HMAC-SHA256", amzDate, scope, sha256(Data(canonical.utf8))].joined(separator: "\n")
    var key = hmac(Data("AWS4\(secret)".utf8), day)
    for part in [region, service, "aws4_request"] { key = hmac(key, part) }
    let signature = hex(hmac(key, toSign))
    return "AWS4-HMAC-SHA256 Credential=\(accessKey)/\(scope), SignedHeaders=\(signed), Signature=\(signature)"
}

func selftest() {
    // AWS SigV4 test suite, "get-vanilla".
    let got = authorization(
        method: "GET", path: "/", query: "",
        headers: ["host": "example.amazonaws.com", "x-amz-date": "20150830T123600Z"],
        payloadHash: sha256(Data()),
        accessKey: "AKIDEXAMPLE", secret: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        region: "us-east-1", service: "service")
    let want = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, "
        + "SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
    if got != want { fail("signer mismatch:\n got  \(got)\n want \(want)") }
    print("ok")
}

// MARK: - R2 requests

struct Bucket {
    let endpoint: URL, bucket: String, accessKey: String, secret: String

    static func fromEnv() -> Bucket {
        let e = ProcessInfo.processInfo.environment
        guard let ep = e["R2_ENDPOINT"], let url = URL(string: ep), url.host != nil,
              let b = e["R2_BUCKET"], let a = e["R2_ACCESS_KEY_ID"], let s = e["R2_SECRET_ACCESS_KEY"]
        else { fail("R2_ENDPOINT, R2_BUCKET, R2_ACCESS_KEY_ID and R2_SECRET_ACCESS_KEY must be set") }
        return Bucket(endpoint: url, bucket: b, accessKey: a, secret: s)
    }

    /// Path-style request: <endpoint>/<bucket>/<key>. Keys are plain ASCII like photos/x.jpg.
    func send(_ method: String, _ key: String, body: Data = Data(), extra: [String: String] = [:]) -> (Int, Data) {
        let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~/")
        let path = "/\(bucket)/\(key)".addingPercentEncoding(withAllowedCharacters: allowed)!
        let hostHeader = endpoint.port.map { "\(endpoint.host!):\($0)" } ?? endpoint.host!

        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(secondsFromGMT: 0)
        f.dateFormat = "yyyyMMdd'T'HHmmss'Z'"
        let payloadHash = sha256(body)
        let signedHeaders = ["host": hostHeader, "x-amz-date": f.string(from: Date()), "x-amz-content-sha256": payloadHash]
        let auth = authorization(method: method, path: path, query: "", headers: signedHeaders,
                                 payloadHash: payloadHash, accessKey: accessKey, secret: secret,
                                 region: "auto", service: "s3")

        // The endpoint is scheme://host[:port] only; the bucket and key form the whole path.
        var req = URLRequest(url: URL(string: "\(endpoint.scheme ?? "https")://\(hostHeader)\(path)")!)
        req.httpMethod = method
        req.timeoutInterval = 60
        for (k, v) in signedHeaders where k != "host" { req.setValue(v, forHTTPHeaderField: k) }
        for (k, v) in extra { req.setValue(v, forHTTPHeaderField: k) }
        req.setValue(auth, forHTTPHeaderField: "Authorization")
        if method == "PUT" { req.httpBody = body }

        let done = DispatchSemaphore(value: 0)
        var result: (Int, Data) = (0, Data())
        var failure: Error?
        URLSession.shared.dataTask(with: req) { data, resp, err in
            failure = err
            result = ((resp as? HTTPURLResponse)?.statusCode ?? 0, data ?? Data())
            done.signal()
        }.resume()
        done.wait()
        if let failure { fail("\(method) \(key): \(failure.localizedDescription)") }
        return result
    }

    func check(_ what: String, _ r: (Int, Data)) {
        guard (200..<300).contains(r.0) else {
            fail("\(what) failed with HTTP \(r.0): \(String(decoding: r.1.prefix(500), as: UTF8.self))")
        }
    }
}

// MARK: - Main

let args = Array(CommandLine.arguments.dropFirst())
switch args.first {
case "info":
    printJSON(args.dropFirst().map { ["takenAt": takenAt($0)] })
case "render" where args.count == 5:
    guard let px = Int(args[4]) else { fail("bad size") }
    render(args[1], args[2], args[3], px)
case "get" where args.count == 3:
    let b = Bucket.fromEnv()
    let r = b.send("GET", args[1])
    if r.0 == 404 { exit(3) }
    b.check("GET \(args[1])", r)
    do { try r.1.write(to: URL(fileURLWithPath: args[2])) } catch { fail("cannot write \(args[2])") }
case "put" where args.count == 5:
    let b = Bucket.fromEnv()
    guard let body = FileManager.default.contents(atPath: args[2]) else { fail("cannot read \(args[2])") }
    b.check("PUT \(args[1])", b.send("PUT", args[1], body: body,
                                    extra: ["Content-Type": args[3], "Cache-Control": args[4]]))
case "delete" where args.count == 2:
    let b = Bucket.fromEnv()
    let r = b.send("DELETE", args[1])
    if r.0 != 404 { b.check("DELETE \(args[1])", r) }
case "selftest":
    selftest()
default:
    fail("usage: publish-helper info|render|get|put|delete|selftest ...", 2)
}

// #359：只读已编译的实验模型；输入是显式给出的 NPY float32 数据区。
import Foundation
import CoreML

@_silgen_name("proc_pid_rusage")
func queryUsage(_ pid: Int32, _ flavor: Int32, _ info: UnsafeMutableRawPointer) -> Int32

struct Config: Decodable {
    let model: String
    let inputs: String
    let offset: Int
    let count: Int
    let output: String
}
struct Invalid: Error { let message: String }
func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw Invalid(message: message) }
}
func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds }
func milliseconds(_ start: UInt64) -> Double { Double(now() - start) / 1e6 }

let config = try JSONDecoder().decode(Config.self, from: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1])))
let destination = URL(fileURLWithPath: config.output)
try require(!FileManager.default.fileExists(atPath: config.output), "output already exists")
let data = try Data(contentsOf: URL(fileURLWithPath: config.inputs), options: .mappedIfSafe)
let inputBytes = 448 * 448 * 3 * MemoryLayout<Float>.size
try require(config.count > 0 && config.offset >= 0 && data.count == config.offset + config.count * inputBytes, "input size mismatch")
let options = MLModelConfiguration()
options.computeUnits = .all
options.allowLowPrecisionAccumulationOnGPU = false
let started = now()
let model = try MLModel(contentsOf: URL(fileURLWithPath: config.model), configuration: options)
let loadMs = milliseconds(started)
let description = model.modelDescription
try require(description.inputDescriptionsByName.count == 1, "expected one input")
let inputName = description.inputDescriptionsByName.keys.first!
let input = try MLMultiArray(shape: [1, 448, 448, 3], dataType: .float32)
try require(input.strides.map(\.intValue) == [602112, 1344, 3, 1], "non-contiguous input")
let provider = try MLDictionaryFeatureProvider(dictionary: [inputName: MLFeatureValue(multiArray: input)])
let outputNames = description.outputDescriptionsByName.keys.sorted()
try require(outputNames.count == 2, "expected two outputs")

func infer(_ index: Int) throws -> (Double, Data, Data) {
    data.withUnsafeBytes { raw in
        input.dataPointer.copyMemory(from: raw.baseAddress! + config.offset + index * inputBytes, byteCount: inputBytes)
    }
    let start = now()
    let features = try model.prediction(from: provider)
    var probability = Data(), embedding = Data()
    for name in outputNames {
        guard let array = features.featureValue(for: name)?.multiArrayValue else { throw Invalid(message: "missing array") }
        try require(array.dataType == .float32 && array.strides.last?.intValue == 1, "output layout changed")
        // 固定模型的输出均为 (1, N)；把 CPU 可读的数据复制出来后才停止计时。
        let shape = array.shape.map(\.intValue)
        try require(shape.count == 2 && shape[0] == 1, "output shape changed")
        let copied = Data(bytes: array.dataPointer, count: array.count * MemoryLayout<Float>.size)
        if array.count == 1024 { embedding = copied } else { probability = copied }
    }
    try require(embedding.count == 4096 && probability.count > embedding.count, "output identity mismatch")
    return (milliseconds(start), probability, embedding)
}

var warmup = [Double]()
for i in 0..<3 { warmup.append(try infer(i % config.count).0) }
var times = [Double](), probabilities = Data(), embeddings = Data()
for i in 0..<config.count {
    let (elapsed, probability, embedding) = try autoreleasepool { try infer(i) }
    times.append(elapsed); probabilities.append(probability); embeddings.append(embedding)
}
try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
try probabilities.write(to: destination.appendingPathComponent("probs.f32"))
try embeddings.write(to: destination.appendingPathComponent("emb.f32"))
var usage = [UInt64](repeating: 0, count: 42)
let success = usage.withUnsafeMutableBytes { queryUsage(getpid(), 4, $0.baseAddress!) }
let sorted = times.sorted()
let result: [String: Any] = ["images": config.count, "load_ms": loadMs, "warmup_ms": warmup,
    "median_ms": sorted[sorted.count / 2], "mean_ms": times.reduce(0,+)/Double(times.count),
    "peak_footprint_mib": success == 0 ? Double(usage[30])/1048576 : -1,
    "input_name": inputName, "output_names": outputNames, "times_ms": times]
let encoded = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
try encoded.write(to: destination.appendingPathComponent("result.json"))
print(String(decoding: encoded, as: UTF8.self))

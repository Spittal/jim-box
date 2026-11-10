import Vision
import CoreImage
import AppKit
// Writes a person mask PNG for a portrait photo, and prints the largest face's
// landmarks (image pixels, y down) as JSON.
//   swift cutout.swift photo.png mask.png > landmarks.json
let url = URL(fileURLWithPath: CommandLine.arguments[1])
let ci = CIImage(contentsOf: url)!
let size = ci.extent.size
let handler = VNImageRequestHandler(ciImage: ci)
let segmentation = VNGeneratePersonSegmentationRequest()
segmentation.qualityLevel = .accurate
segmentation.outputPixelFormat = kCVPixelFormatType_OneComponent8
let faces = VNDetectFaceLandmarksRequest()
try handler.perform([segmentation, faces])
let mask = CIImage(cvPixelBuffer: segmentation.results!.first!.pixelBuffer)
let scaled = mask.transformed(by: CGAffineTransform(scaleX: size.width / mask.extent.width, y: size.height / mask.extent.height))
try NSBitmapImageRep(ciImage: scaled).representation(using: .png, properties: [:])!
  .write(to: URL(fileURLWithPath: CommandLine.arguments[2]))
let face = faces.results!.max { $0.boundingBox.width < $1.boundingBox.width }!
let marks = face.landmarks!
func points(_ region: VNFaceLandmarkRegion2D?) -> [[Double]] {
  (region?.pointsInImage(imageSize: size) ?? []).map { [Double($0.x), Double(size.height - $0.y)] }
}
let box = face.boundingBox
let out: [String: Any] = [
  "roll": face.roll?.doubleValue ?? 0,
  "box": [box.minX * size.width, (1 - box.maxY) * size.height, box.width * size.width, box.height * size.height],
  "contour": points(marks.faceContour), "leftPupil": points(marks.leftPupil), "rightPupil": points(marks.rightPupil),
  "outerLips": points(marks.outerLips),
]
print(String(data: try JSONSerialization.data(withJSONObject: out, options: [.sortedKeys]), encoding: .utf8)!)

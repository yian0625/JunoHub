export async function compressAvatar(file: File): Promise<string> {
  if (!/^image\/(png|jpeg|webp)$/.test(file.type)) throw new Error("请选择 PNG、JPEG 或 WebP 图片。")
  if (file.size > 8 * 1024 * 1024) throw new Error("原始图片不能超过 8MB。")
  const bitmap = await createImageBitmap(file)
  const canvas = document.createElement("canvas")
  canvas.width = 256; canvas.height = 256
  const context = canvas.getContext("2d")
  if (!context) { bitmap.close(); throw new Error("当前浏览器无法处理头像图片。") }
  const side = Math.min(bitmap.width, bitmap.height)
  context.drawImage(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side, 0, 0, 256, 256)
  bitmap.close()
  for (const quality of [0.85, 0.7, 0.55]) {
    const blob = await canvasBlob(canvas, quality)
    if (blob.size <= 256 * 1024) return readDataUrl(blob)
  }
  throw new Error("无法将头像压缩到 256KB 以内，请选择其他图片。")
}

function canvasBlob(canvas: HTMLCanvasElement, quality: number): Promise<Blob> {
  return new Promise((resolve, reject) => canvas.toBlob(blob => blob ? resolve(blob) : reject(new Error("无法处理头像图片。")), "image/webp", quality))
}

function readDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result))
    reader.onerror = () => reject(new Error("无法读取头像图片。"))
    reader.readAsDataURL(blob)
  })
}

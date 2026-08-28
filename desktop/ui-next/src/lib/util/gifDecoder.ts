// GIF 帧解码器:将 GIF/PNG 转为 PNG data URL。
// GIF 通过浏览器 Canvas 渲染(避免手写 LZW 解码器的各种边界 bug)。
// PNG 直接返回不转换。
// 静态/动态 GIF 都能正确显示——浏览器 Image 元素渲染 GIF 的第一帧。

const TARGET_FRAME_SIZE = 400;

/**
 * 将 GIF/PNG data URL 转为 PNG data URL。
 * - PNG:直接返回
 * - GIF:渲染到 Canvas → 输出 PNG(浏览器保证 alpha/颜色正确)
 */
export async function decodeGifToSpriteSheet(dataUrl: string): Promise<string> {
  if (dataUrl.startsWith("data:image/png")) return dataUrl;
  // 非 PNG → 当作 GIF/其他格式,用 Canvas 渲染
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const w = img.naturalWidth || TARGET_FRAME_SIZE;
      const h = img.naturalHeight || TARGET_FRAME_SIZE;
      const canvas = document.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      const ctx = canvas.getContext("2d")!;
      // 清除为透明背景(不是黑色)
      ctx.clearRect(0, 0, w, h);
      ctx.drawImage(img, 0, 0, w, h);
      const png = canvas.toDataURL("image/png");
      // 基本验证:PNG 不能太小(至少 1KB 才合理)
      if (png.length < 1000) {
        reject(new Error("渲染结果异常(可能 GIF 格式不支持)"));
        return;
      }
      resolve(png);
    };
    img.onerror = () => reject(new Error("图片加载失败,请确认文件格式"));
    img.src = dataUrl;
  });
}

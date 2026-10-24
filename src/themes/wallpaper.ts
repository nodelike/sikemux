/** A wallpaper's pixels as RGBA bytes, at most `size` points on the long side, for the core to pick a theme from. */
export async function wallpaperPixels(dataUrl: string, size = 160): Promise<Uint8ClampedArray> {
    const image = new Image();
    image.src = dataUrl;
    await image.decode();
    const scale = Math.min(1, size / Math.max(image.naturalWidth, image.naturalHeight));
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
    canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
    const context = canvas.getContext("2d");
    if (!context) throw new Error("could not read the wallpaper's pixels");
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
    return context.getImageData(0, 0, canvas.width, canvas.height).data;
}

/** RGBA bytes as base64, as the core takes them. */
export function pixelsBase64(pixels: Uint8ClampedArray): string {
    let text = "";
    for (let at = 0; at < pixels.length; at += 0x8000) text += String.fromCharCode(...pixels.subarray(at, at + 0x8000));
    return btoa(text);
}

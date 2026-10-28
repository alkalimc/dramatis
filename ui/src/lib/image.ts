import type { ImagePart } from "../api";

export function readImage(file: Blob): Promise<ImagePart> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onerror = () => reject(r.error);
    r.onload = () => {
      const url = String(r.result);
      resolve({ mime: file.type, data_base64: url.slice(url.indexOf(",") + 1) });
    };
    r.readAsDataURL(file);
  });
}

/** Longest side of a stored picture, in pixels: avatars are shown small. */
const AVATAR_EDGE = 320;

/** A picture for an avatar: cropped square from the middle and scaled down, so it stays light. */
export async function readAvatar(file: Blob): Promise<ImagePart> {
  try {
    const bitmap = await createImageBitmap(file);
    const side = Math.min(bitmap.width, bitmap.height);
    const edge = Math.min(side, AVATAR_EDGE);
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = edge;
    const ctx = canvas.getContext("2d");
    if (!ctx) return await readImage(file);
    ctx.drawImage(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side, 0, 0, edge, edge);
    const url = canvas.toDataURL("image/png");
    return { mime: "image/png", data_base64: url.slice(url.indexOf(",") + 1) };
  } catch {
    return readImage(file);
  }
}

/** The first image on the clipboard, if any. */
export function pastedImage(data: DataTransfer | null): File | null {
  for (const item of Array.from(data?.items ?? [])) {
    if (item.kind === "file" && item.type.startsWith("image/")) return item.getAsFile();
  }
  return null;
}

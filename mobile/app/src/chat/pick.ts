import * as DocumentPicker from 'expo-document-picker';
import { File } from 'expo-file-system';
import { ImageManipulator, SaveFormat } from 'expo-image-manipulator';
import * as ImagePicker from 'expo-image-picker';
import { Asset } from 'expo-media-library';

import type { LibraryPhoto } from './library';
import type { Attachment } from './uploads';

/** A photo is sent no larger than this on its longest side, which agents read at full detail. */
const LONGEST_EDGE = 2048;
/** Pictures the transcript can draw from this phone; others are sent as files. */
const DRAWN = new Set(['image/jpeg', 'image/png', 'image/gif', 'image/webp']);

let picks = 0;
const nextId = () => `pick-${(picks += 1)}`;

function stem(name: string | null | undefined): string | undefined {
  return name?.replace(/\.[^.]*$/, '') || undefined;
}

function sizeOf(uri: string): number {
  return new File(uri).size ?? 0;
}

type Photo = { uri: string; width: number; height: number; name: string | null | undefined };

const fromPicker = (asset: ImagePicker.ImagePickerAsset): Photo => ({ ...asset, name: asset.fileName });

/** Picked in the system's photo picker, which needs no access to the library. */
export async function pickPhotos(limit: number): Promise<Attachment[]> {
  const result = await ImagePicker.launchImageLibraryAsync({
    mediaTypes: ['images'],
    allowsMultipleSelection: true,
    selectionLimit: limit,
    quality: 1,
    exif: false,
    preferredAssetRepresentationMode: ImagePicker.UIImagePickerPreferredAssetRepresentationMode.Compatible,
  });
  if (result.canceled) return [];
  const photos: Attachment[] = [];
  // One at a time, since each full-size photo is decoded in memory.
  for (const asset of result.assets.slice(0, limit)) photos.push(await asJpeg(fromPicker(asset)));
  return photos;
}

/** Photos chosen in the app's own gallery, in the order they were chosen. */
export async function libraryPhotos(chosen: readonly LibraryPhoto[]): Promise<Attachment[]> {
  const photos: Attachment[] = [];
  for (const { id, width, height, filename } of chosen) {
    const asset = new Asset(id);
    photos.push(
      await asJpeg({
        uri: await asset.getUri(),
        width: width ?? (await asset.getWidth()),
        height: height ?? (await asset.getHeight()),
        name: filename,
      }),
    );
  }
  return photos;
}

export async function takePhoto(): Promise<Attachment[]> {
  const permission = await ImagePicker.requestCameraPermissionsAsync();
  if (!permission.granted) throw new Error('Sikemux has no access to the camera; allow it in Settings');
  const result = await ImagePicker.launchCameraAsync({ mediaTypes: ['images'], quality: 1, exif: false });
  if (result.canceled) return [];
  return [await asJpeg(fromPicker(result.assets[0]))];
}

/** Scaled down and as JPEG, which every agent reads (HEIC included). */
async function asJpeg(photo: Photo): Promise<Attachment> {
  const context = ImageManipulator.manipulate(photo.uri);
  if (Math.max(photo.width, photo.height) > LONGEST_EDGE) {
    context.resize(photo.width >= photo.height ? { width: LONGEST_EDGE } : { height: LONGEST_EDGE });
  }
  const image = await context.renderAsync();
  const saved = await image.saveAsync({ format: SaveFormat.JPEG, compress: 0.85 });
  image.release();
  context.release();
  return {
    id: nextId(),
    kind: 'image',
    name: `${stem(photo.name) ?? 'photo'}.jpg`,
    mime: 'image/jpeg',
    uri: saved.uri,
    size: sizeOf(saved.uri),
  };
}

export async function pickFiles(limit: number): Promise<Attachment[]> {
  const result = await DocumentPicker.getDocumentAsync({ type: '*/*', multiple: true, copyToCacheDirectory: true });
  if (result.canceled) return [];
  return result.assets.slice(0, limit).map((asset) => {
    const mime = asset.mimeType ?? 'application/octet-stream';
    return {
      id: nextId(),
      kind: DRAWN.has(mime) ? 'image' : 'file',
      name: asset.name,
      mime,
      uri: asset.uri,
      size: asset.size ?? sizeOf(asset.uri),
    };
  });
}

export function sizeLabel(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

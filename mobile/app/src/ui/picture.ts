import type { ExpoWebGLRenderingContext } from 'expo-gl';

/** expo-gl uploads a file straight from its URI, so the texture is ready at once. */
export function uploadPicture(gl: ExpoWebGLRenderingContext, uri: string): Promise<void> | undefined {
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, { localUri: uri } as never);
  return undefined;
}

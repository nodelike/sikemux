import type { ExpoWebGLRenderingContext } from 'expo-gl';

/** A browser's WebGL takes a decoded image rather than a URI, so the texture waits for it to load. */
export function uploadPicture(gl: ExpoWebGLRenderingContext, uri: string): Promise<void> | undefined {
  return new Promise((done) => {
    const image = new window.Image();
    image.onload = () => {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, image);
      done();
    };
    image.onerror = () => done();
    image.src = uri;
  });
}

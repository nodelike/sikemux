declare module "*.ttf" {
  const bytes: Uint8Array;
  export default bytes;
}
declare module "*.wasm" {
  const bytes: Uint8Array;
  export default bytes;
}
declare module "*.svg" {
  const text: string;
  export default text;
}

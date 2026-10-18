export const AssetField = { CREATION_TIME: 'creationTime', MEDIA_TYPE: 'mediaType' } as const;
export const MediaType = { IMAGE: 'image' } as const;

export class Query {
  eq() {
    return this;
  }
  orderBy() {
    return this;
  }
  limit() {
    return this;
  }
  offset() {
    return this;
  }
  async exeForMetadata() {
    return [];
  }
}

export class Asset {
  constructor(readonly id: string) {}
}

export const requestPermissionsAsync = async () => ({ granted: false, accessPrivileges: 'none' });
export const presentPermissionsPicker = async () => {};
export const addListener = () => ({ remove() {} });

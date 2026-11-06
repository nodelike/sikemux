export const WHEN_UNLOCKED_THIS_DEVICE_ONLY = 'WHEN_UNLOCKED_THIS_DEVICE_ONLY';
export const AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY = 'AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY';

const store = new Map<string, string>();

export const getItemAsync = async (key: string) => store.get(key) ?? null;
export const setItemAsync = async (key: string, value: string) => void store.set(key, value);
export const deleteItemAsync = async (key: string) => void store.delete(key);
export const getItem = (key: string) => store.get(key) ?? null;
export const setItem = (key: string, value: string) => void store.set(key, value);

export const clear = () => store.clear();

let text = '';

export const getStringAsync = async () => text;
export const setStringAsync = async (value: string) => {
  text = value;
  return true;
};
export const hasStringAsync = async () => text.length > 0;

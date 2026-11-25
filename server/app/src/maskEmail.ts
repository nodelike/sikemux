/** An address as a person recognises their own, without showing it to whoever else sees the page. */
export const maskEmail = (email: string) =>
  email.replace(/^(.)[^@]*(?=@)/, (_, first: string) => `${first}•••`);

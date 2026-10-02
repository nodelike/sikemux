const noop = () => {};

export const router = { push: noop, replace: noop, back: noop, dismiss: noop, dismissAll: noop, navigate: noop, canGoBack: () => false };
export const useRouter = () => router;
export const useLocalSearchParams = () => ({});

const signozLogo = new URL("../signoz.svg", import.meta.url).href;

export function SignozIcon({ size = 14 }: { size?: number }) {
    return <img src={signozLogo} width={size} height={size} alt="" aria-hidden="true" />;
}

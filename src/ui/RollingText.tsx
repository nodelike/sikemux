import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { animate, canAnimate } from "../lib/motion";

/** Text whose changed characters roll up into place, such as a clock ticking over. It mounts still. */
export function RollingText({ text }: { text: string }) {
    const box = useRef<HTMLSpanElement>(null);
    const last = useRef<string | null>(null);
    useLayoutEffect(() => {
        const previous = last.current;
        last.current = text;
        if (previous === null || previous === text || !box.current) return;
        [...box.current.children].forEach((char, i) => {
            if (previous[i] !== text[i])
                animate(
                    char,
                    [
                        { opacity: 0, transform: "translateY(0.55em)" },
                        { opacity: 1, transform: "none" },
                    ],
                    { duration: 200 },
                );
        });
    }, [text]);
    return (
        <span ref={box} className="rolling-text">
            {[...text].map((char, i) => (
                <span key={i}>{char}</span>
            ))}
        </span>
    );
}

/** A number that counts to its new value when it changes. It mounts on its value. */
export function CountUp({ value }: { value: number }) {
    const [shown, setShown] = useState(value);
    const settled = useRef(value);
    useEffect(() => {
        const from = settled.current;
        settled.current = value;
        if (from === value) return;
        const start = performance.now();
        let frame = 0;
        const step = (now: number) => {
            const k = Math.min(1, (now - start) / 320);
            setShown(Math.round(from + (value - from) * (1 - (1 - k) ** 3)));
            if (k < 1) frame = requestAnimationFrame(step);
        };
        frame = requestAnimationFrame(step);
        return () => cancelAnimationFrame(frame);
    }, [value]);
    // Without motion the number is simply the value, with no frame spent on the old one.
    return <>{canAnimate(document.body) ? shown : value}</>;
}

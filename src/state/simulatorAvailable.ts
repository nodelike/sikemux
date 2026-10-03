import { useEffect, useState } from "react";
import { simulatorApi } from "../api/simulator";

let asked: Promise<boolean> | null = null;

/** Whether this Mac can run iOS simulators. Asked once; Xcode does not come and go while Sikemux runs. */
export function useSimulatorsAvailable(): boolean {
    const [available, setAvailable] = useState(false);
    useEffect(() => {
        let current = true;
        asked ??= simulatorApi.available().catch(() => {
            asked = null;
            return false;
        });
        void asked.then((answer) => current && setAvailable(answer));
        return () => {
            current = false;
        };
    }, []);
    return available;
}

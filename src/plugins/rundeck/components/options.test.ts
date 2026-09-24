import { describe, expect, it } from "vitest";
import type { JobOption } from "../api";
import { initialOptionValues, referencedOptions, reusableOptions, runOptionValues, substituteOptionRefs, validateOptions } from "./options";

function option(name: string, extra: Partial<JobOption> = {}): JobOption {
    return {
        name,
        label: null,
        description: null,
        required: false,
        secure: false,
        value_exposed: false,
        default: null,
        values: null,
        values_url: null,
        enforced: false,
        multivalued: false,
        delimiter: null,
        is_date: false,
        date_format: null,
        kind: "text",
        ...extra,
    };
}

describe("initialOptionValues", () => {
    it("prefills defaults, then the previous run, then the branch, and never secrets", () => {
        const options = [
            option("BRANCH", { default: "main" }),
            option("REGION", { default: "eu" }),
            option("PASSWORD", { secure: true, default: "x" }),
        ];
        expect(initialOptionValues(options, { REGION: "us", PASSWORD: "leak" }, "BRANCH", "feature/y")).toEqual({
            BRANCH: "feature/y",
            REGION: "us",
            PASSWORD: "",
        });
        expect(initialOptionValues(options, undefined, "BRANCH", "")).toEqual({ BRANCH: "main", REGION: "eu", PASSWORD: "" });
    });
});

describe("remote option values", () => {
    it("fills option references from the form", () => {
        const url = "https://x/values?env=${option.ENV.value}&team=${option.TEAM.value}";
        expect(referencedOptions(url)).toEqual(["ENV", "TEAM"]);
        expect(substituteOptionRefs(url, { ENV: "prod eu", TEAM: "" })).toBe("https://x/values?env=prod%20eu&team=");
    });
});

describe("validateOptions", () => {
    const options = [
        option("BRANCH", { required: true }),
        option("SIZE", { enforced: true, values: ["s", "m"] }),
        option("TAGS", { enforced: true, multivalued: true, delimiter: "|", values: ["a", "b"] }),
        option("REMOTE", { enforced: true, values_url: "https://x" }),
        option("UPLOAD", { kind: "file", required: true }),
    ];

    it("reports required, enforced and file options per field", () => {
        expect(validateOptions(options, { BRANCH: " ", SIZE: "xl", TAGS: "a|c", REMOTE: "z" }, { REMOTE: ["y"] })).toEqual({
            BRANCH: "required",
            SIZE: '"xl" is not one of the allowed values',
            TAGS: '"c" is not one of the allowed values',
            REMOTE: '"z" is not one of the allowed values',
            UPLOAD: "file options must be run from Rundeck",
        });
    });

    it("skips enforcement when remote values could not be loaded", () => {
        const errors = validateOptions(options.slice(0, 4), { BRANCH: "main", SIZE: "m", TAGS: "a|b", REMOTE: "z" }, { REMOTE: null });
        expect(errors).toEqual({});
    });
});

describe("run and reuse", () => {
    it("sends filled options except files", () => {
        const options = [option("A"), option("B"), option("F", { kind: "file" })];
        expect(runOptionValues(options, { A: " 1 ", B: "", F: "x" })).toEqual({ A: "1" });
    });

    it("drops secure options from a past run", () => {
        expect(reusableOptions({ A: "1", SECRET: "s" }, new Set(["SECRET"]))).toEqual({ A: "1" });
    });
});

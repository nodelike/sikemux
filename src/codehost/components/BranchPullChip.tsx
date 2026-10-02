import { Tooltip } from "../../plugin-api/ui";
import type { Pull } from "../types";
import { stateOf } from "./Bits";
import { SectionIcon } from "./ActionsIcon";

/** A branch's open pull request, beside the branch in the git pane's list. */
export function BranchPullChip({ pull, onOpen }: { pull: Pull; onOpen: () => void }) {
    return (
        <Tooltip label={`#${pull.number} ${pull.title}`}>
            <button
                type="button"
                tabIndex={-1}
                className="git-pull-chip"
                data-state={stateOf(pull.state, pull.draft)}
                onClick={(event) => {
                    event.stopPropagation();
                    onOpen();
                }}>
                <SectionIcon section="pulls" size={11} />#{pull.number}
            </button>
        </Tooltip>
    );
}

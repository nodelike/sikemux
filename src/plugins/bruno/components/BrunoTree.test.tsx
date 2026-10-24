import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { BrunoTree } from "./BrunoTree";

it("activates request rows and expands folders from the keyboard", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(
        <BrunoTree
            paneId="pane"
            collectionPath="/api"
            tree={[
                {
                    type: "folder",
                    name: "users",
                    path: "/api/users",
                    children: [
                        {
                            type: "request",
                            name: "list",
                            path: "/api/users/list.bru",
                            method: "get",
                            collectionPath: "/api",
                        },
                    ],
                },
            ]}
            activePath={null}
            drafts={{}}
            running={{}}
            loading={false}
            error={null}
            onSelect={onSelect}
            onReload={() => {}}
        />,
    );

    const folder = screen.getByRole("button", { name: /users/i });
    folder.focus();
    await user.keyboard("{Enter}");
    expect(folder).toHaveAttribute("aria-expanded", "true");

    const request = screen.getByRole("button", { name: /list/i });
    request.focus();
    await user.keyboard(" ");
    expect(onSelect).toHaveBeenCalledWith("/api/users/list.bru");
});

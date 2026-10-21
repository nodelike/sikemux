import { docker } from "./resources";
import { confirmDialog, notify, runInTerminal, swallow } from "../../plugin-api/host";
import { errorText, formatBytes } from "../../plugin-api/format";
import { dockerApi, isPluginFailure, type Image, type Volume } from "./api";

export function imageName(image: Pick<Image, "tags" | "id">): string {
    return image.tags[0] ?? image.id;
}

export async function removeImage(image: Image): Promise<void> {
    const name = imageName(image);
    const sure = await confirmDialog({
        title: `Remove ${name}?`,
        body:
            image.containers > 0
                ? `${image.containers} stopped container${image.containers === 1 ? " was" : "s were"} made from it; they go with it.`
                : undefined,
        confirmLabel: "Remove",
        destructive: true,
    });
    if (!sure) return;
    try {
        await dockerApi.removeImage(name, image.containers > 0);
        notify("success", `Removed ${name}`);
    } catch (error) {
        notify("error", `Remove ${name}: ${errorText(error)}`);
    } finally {
        docker.refresh();
    }
}

/** A throwaway container of the image in a terminal, gone when its shell exits. */
export function runImage(image: Image): void {
    const name = imageName(image);
    void runInTerminal(`Run ${name}`, ["docker", "run", "--rm", "-it", name]).catch(swallow("run the image"));
}

export async function pruneImages(): Promise<void> {
    const sure = await confirmDialog({
        title: "Remove untagged images?",
        body: "Only images no tag points to and no container uses, such as the old copy left when a tag is built again.",
        confirmLabel: "Remove",
    });
    if (!sure) return;
    try {
        const pruned = await dockerApi.pruneImages();
        notify(
            "success",
            pruned.deleted === 0 ? "Nothing to remove" : `Removed ${pruned.deleted} images, freeing ${formatBytes(pruned.spaceReclaimed)}`,
        );
    } catch (error) {
        notify("error", `Remove untagged images: ${errorText(error)}`);
    } finally {
        docker.refresh();
    }
}

export async function removeVolume(volume: Volume): Promise<void> {
    const sure = await confirmDialog({
        title: `Remove the volume ${volume.name}?`,
        body: "Everything stored in it is deleted, and can't be brought back.",
        confirmLabel: "Delete its data",
        destructive: true,
    });
    if (!sure) return;
    try {
        await dockerApi.removeVolume(volume.name);
        notify("success", `Removed ${volume.name}`);
    } catch (error) {
        notify("error", isPluginFailure(error, "conflict") ? errorText(error) : `Remove ${volume.name}: ${errorText(error)}`);
    } finally {
        docker.refresh();
    }
}

export async function pruneVolumes(): Promise<void> {
    const sure = await confirmDialog({
        title: "Remove unused anonymous volumes?",
        body: "Only volumes no container uses that were never given a name. Named ones stay, since a Compose stack looks for them again.",
        confirmLabel: "Remove",
        destructive: true,
    });
    if (!sure) return;
    try {
        const pruned = await dockerApi.pruneVolumes();
        notify(
            "success",
            pruned.deleted.length === 0
                ? "Nothing to remove"
                : `Removed ${pruned.deleted.length} volumes, freeing ${formatBytes(pruned.spaceReclaimed)}`,
        );
    } catch (error) {
        notify("error", `Remove unused volumes: ${errorText(error)}`);
    } finally {
        docker.refresh();
    }
}

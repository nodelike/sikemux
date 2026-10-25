import { homedir } from 'node:os';
import { join } from 'node:path';

/** The picture behind panes in every showcase and design page, so they all show the same one. */
export const PANE_IMAGE_FILE = join(homedir(), 'wallpaper/midnight-garden.jpeg');

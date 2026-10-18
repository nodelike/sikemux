import { invokeCommand as invoke } from "./invoke";

export interface Skill {
    name: string;
    description: string;
    path: string;
}

/** Skills an agent keeps in its own global folder, which Sikemux only reads. */
export interface OwnSkills {
    agent: string;
    folder: string;
    skills: Skill[];
}

export interface SkillsOverview {
    library: string;
    skills: Skill[];
    own: OwnSkills[];
    clashes: string[];
}

export const skillsApi = {
    overview: () => invoke<SkillsOverview>("skills_overview"),
    create: (name: string, description: string, body: string) => invoke<Skill>("skills_create", { name, description, body }),
    install: (source: string, subfolder?: string) => invoke<Skill>("skills_install", { source, subfolder: subfolder || null }),
    remove: (name: string) => invoke<void>("skills_remove", { name }),
    adopt: (path: string) => invoke<Skill>("skills_adopt", { path }),
};

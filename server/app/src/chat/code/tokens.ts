/* One run of a code fence that reads as one colour, and the lines they make up.
   A run with nothing set is plain text, which keeps the colour the stylesheet
   already gives the fence. */
export interface CodeToken {
  readonly text: string;
  readonly color?: string;
  readonly italic?: boolean;
  readonly bold?: boolean;
  readonly underline?: boolean;
}

export type CodeLine = readonly CodeToken[];

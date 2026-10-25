export type QuestionAnswer =
    { readonly answer: string } | { readonly choice: number } | { readonly choices: readonly number[] } | { readonly fine: true };

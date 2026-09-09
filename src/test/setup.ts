import "@testing-library/jest-dom/vitest";

// jsdom implements no scrolling, so components that keep a selection in view
// would throw here rather than in a browser.
if (!Element.prototype.scrollIntoView) {
    Element.prototype.scrollIntoView = () => {};
}

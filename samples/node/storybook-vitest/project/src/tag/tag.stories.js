import { expect } from "storybook/test";
import { tag } from "./tag.js";

// This story is broken on purpose (the tag shouts): the scenarios show that a story nobody
// touched, and no component it imports, never runs at stop.
export default { title: "Tag", render: tag };

export const Shouting = {
  args: { text: "new" },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.textContent).toBe("new");
  },
};

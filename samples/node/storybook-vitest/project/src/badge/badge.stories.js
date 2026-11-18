import { expect } from "storybook/test";
import { badge } from "./badge.js";

export default { title: "Badge", render: badge };

export const Small = {
  args: { count: 5 },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.textContent).toBe("5");
  },
};

export const Capped = {
  args: { count: 120 },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.textContent).toBe("99+");
  },
};

import { expect, test } from "vitest";
import { welcome } from "../src/main";

test("welcomes loudly", () => {
  expect(welcome("Ada")).toBe("HELLO, ADA!");
});

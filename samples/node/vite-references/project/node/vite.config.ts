// The config project has errors of its own that predate any edit: no Node types are installed.
import { resolve } from "node:path";

export default {
  root: resolve("../src"),
  port: "3000" as unknown as number,
  build: { target: 2023 as string },
};

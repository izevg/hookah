export default {
  stories: ["../src/**/*.stories.js"],
  addons: ["@storybook/addon-vitest"],
  framework: "@storybook/html-vite",
  core: { disableTelemetry: true },
};

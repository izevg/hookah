/** The domain layer must stay free of infrastructure. */
module.exports = {
  forbidden: [
    {
      name: "domain-not-to-infra",
      severity: "error",
      from: { path: "^src/domain" },
      to: { path: "^src/infra" },
    },
  ],
  options: { tsConfig: { fileName: "tsconfig.json" } },
};

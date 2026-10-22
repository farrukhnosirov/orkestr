// Metro: consume @orkestr/client and @orkestr/protocol from source in
// ../../packages while resolving every other module (React!) from this app's
// own node_modules — the root workspace uses React 18, Expo needs React 19.
const path = require("node:path");
const { getDefaultConfig } = require("expo/metro-config");

const projectRoot = __dirname;
const packages = path.resolve(projectRoot, "../../packages");
const config = getDefaultConfig(projectRoot);

config.watchFolders = [packages];
config.resolver.nodeModulesPaths = [path.resolve(projectRoot, "node_modules")];
config.resolver.disableHierarchicalLookup = true;
config.resolver.extraNodeModules = {
  "@orkestr/client": path.join(packages, "client/src"),
  "@orkestr/protocol": path.join(packages, "protocol/src"),
};

module.exports = config;

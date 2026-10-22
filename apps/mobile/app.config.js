// Dynamic additions on top of app.json: Firebase config only when present
// (real FCM delivery is optional; see ADR-0037).
const fs = require("node:fs");
const path = require("node:path");

module.exports = ({ config }) => {
  const gsf = path.join(__dirname, "google-services.json");
  if (fs.existsSync(gsf)) {
    config.android = { ...config.android, googleServicesFile: "./google-services.json" };
  }
  return config;
};

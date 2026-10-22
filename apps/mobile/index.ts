// Entry: Hermes polyfills first, then the crypto provider, then the app.
import "./src/polyfills";
import { nobleCrypto, setRemoteCrypto } from "@orkestr/client";
import { registerRootComponent } from "expo";

import App from "./App";

setRemoteCrypto(nobleCrypto());
registerRootComponent(App);

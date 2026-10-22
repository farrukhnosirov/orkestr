import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";

import { App } from "./App";
import { buildResources, i18n } from "./i18n";

describe("web skeleton", () => {
  it("renders the placeholder", async () => {
    await i18n.changeLanguage("en");
    render(
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>,
    );
    expect(screen.getByRole("heading").textContent).toBe("Orkestr");
  });

  it("ships en and ru with the same keys", () => {
    const res = buildResources();
    expect(Object.keys(res).sort()).toEqual(["en", "ru"]);
    expect(Object.keys(res.ru?.common ?? {})).toEqual(Object.keys(res.en?.common ?? {}));
  });
});

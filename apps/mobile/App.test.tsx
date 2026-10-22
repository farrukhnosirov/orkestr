import { render, screen } from "@testing-library/react-native";

import App from "./App";

test("renders the placeholder shell", async () => {
  await render(<App />);
  expect(screen.getByTestId("app-root")).toBeTruthy();
  expect(screen.getByText("Orkestr")).toBeTruthy();
});

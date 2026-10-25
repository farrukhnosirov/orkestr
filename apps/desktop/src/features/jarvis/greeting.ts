/**
 * Greeting of the day (ADR-0049, setting «Приветствие при запуске»): at app
 * start, and — when the app ran overnight — before the first reply of a new
 * day. Once per calendar day on this device.
 */
import { firstGreetingOfDay } from "./persona";
import { useJarvisStore } from "./store";

/** Greeting enabled (only once the settings are loaded). */
export function greetingEnabled(): boolean {
  return useJarvisStore.getState().config?.greetOnStart === true;
}

/** «Доброе утро, сэр.» when the user was not greeted today, else null. */
export function dailyGreeting(now: Date = new Date()): string | null {
  if (!greetingEnabled()) return null;
  return firstGreetingOfDay(now);
}

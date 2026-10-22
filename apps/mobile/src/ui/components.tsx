/** Small dark UI kit for the mobile screens. Every interactive element takes a `testID`. */
import type { ReactNode } from "react";
import { ActivityIndicator, Modal, Pressable, ScrollView, StyleSheet, Switch, Text, TextInput, type TextInputProps, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { colors, radius, space } from "./theme";

export function Screen({ children, testID, scroll = true, top = false }: { children: ReactNode; testID: string; scroll?: boolean; top?: boolean }) {
  const insets = useSafeAreaInsets();
  const pad = { paddingTop: top ? insets.top + space.lg : space.lg, paddingBottom: insets.bottom + space.xl * 2 };
  return (
    <View style={[s.screen, { paddingLeft: insets.left, paddingRight: insets.right }]} testID={testID}>
      {scroll ? (
        <ScrollView contentContainerStyle={[s.content, pad]} keyboardShouldPersistTaps="handled">
          {children}
        </ScrollView>
      ) : (
        <View style={[s.content, pad, { flex: 1 }]}>{children}</View>
      )}
    </View>
  );
}

export function Card({ children, title, testID }: { children: ReactNode; title?: string; testID?: string }) {
  return (
    <View style={s.card} testID={testID}>
      {title ? <Text style={s.cardTitle}>{title}</Text> : null}
      {children}
    </View>
  );
}

type Variant = "primary" | "secondary" | "danger" | "ghost";

export function Button(p: { label: string; onPress: () => void; testID: string; variant?: Variant; disabled?: boolean; busy?: boolean; big?: boolean }) {
  const v = p.variant ?? "primary";
  const disabled = p.disabled || p.busy;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={p.label}
      accessibilityState={{ disabled: !!disabled }}
      testID={p.testID}
      disabled={disabled}
      onPress={p.onPress}
      style={({ pressed }) => [s.btn, s[`btn_${v}`], p.big && s.btnBig, (pressed || disabled) && { opacity: disabled ? 0.45 : 0.75 }]}
    >
      {p.busy ? (
        <ActivityIndicator color={v === "primary" ? colors.inverse : colors.text} />
      ) : (
        <Text style={[s.btnText, v === "primary" && { color: colors.inverse }, p.big && s.btnBigText]}>{p.label}</Text>
      )}
    </Pressable>
  );
}

export function Field(p: TextInputProps & { label: string; testID: string }) {
  const { label, style, ...rest } = p;
  return (
    <View style={{ marginBottom: space.md }}>
      <Text style={s.label}>{label}</Text>
      <TextInput placeholderTextColor={colors.subtle} style={[s.input, style]} autoCapitalize="none" autoCorrect={false} {...rest} />
    </View>
  );
}

export function Toggle(p: { label: string; value: boolean; onChange: (v: boolean) => void; testID: string; hint?: string }) {
  return (
    <View style={s.row}>
      <View style={{ flex: 1, paddingRight: space.md }}>
        <Text style={s.text}>{p.label}</Text>
        {p.hint ? <Text style={s.muted}>{p.hint}</Text> : null}
      </View>
      <Switch
        testID={p.testID}
        value={p.value}
        onValueChange={p.onChange}
        trackColor={{ true: colors.accentStrong, false: colors.border }}
        thumbColor={colors.text}
      />
    </View>
  );
}

export function H1({ children }: { children: ReactNode }) {
  return <Text style={s.h1}>{children}</Text>;
}
export function Muted({ children, testID }: { children: ReactNode; testID?: string }) {
  return (
    <Text style={s.muted} testID={testID}>
      {children}
    </Text>
  );
}
export function Body({ children, testID }: { children: ReactNode; testID?: string }) {
  return (
    <Text style={s.text} testID={testID}>
      {children}
    </Text>
  );
}
export function ErrorText({ children, testID = "error" }: { children: ReactNode; testID?: string }) {
  return children ? (
    <Text style={s.error} testID={testID}>
      {children}
    </Text>
  ) : null;
}
export function Badge({ label, tone = "muted" }: { label: string; tone?: "ok" | "danger" | "warn" | "muted" | "accent" }) {
  const c = { ok: colors.success, danger: colors.danger, warn: colors.warning, muted: colors.subtle, accent: colors.accent }[tone];
  return <Text style={[s.badge, { color: c, borderColor: c }]}>{label}</Text>;
}

export function ListItem(p: { title: string; subtitle?: string; right?: ReactNode; onPress?: () => void; testID: string }) {
  return (
    <Pressable testID={p.testID} onPress={p.onPress} disabled={!p.onPress} style={({ pressed }) => [s.item, pressed && { opacity: 0.7 }]}>
      <View style={{ flex: 1 }}>
        <Text style={s.itemTitle} numberOfLines={2}>
          {p.title}
        </Text>
        {p.subtitle ? (
          <Text style={s.muted} numberOfLines={3}>
            {p.subtitle}
          </Text>
        ) : null}
      </View>
      {p.right}
    </Pressable>
  );
}

/** Modal confirmation (emergency stop, kill switch, revoke, voice commands). */
export function ConfirmDialog(p: {
  visible: boolean;
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
  danger?: boolean;
  testID: string;
}) {
  return (
    <Modal visible={p.visible} transparent animationType="fade" onRequestClose={p.onCancel}>
      <View style={s.backdrop}>
        <View style={s.dialog} testID={p.testID}>
          <Text style={s.h2}>{p.title}</Text>
          <Text style={[s.text, { marginBottom: space.lg }]}>{p.message}</Text>
          <Button label={p.confirmLabel} onPress={p.onConfirm} testID={`${p.testID}-confirm`} variant={p.danger ? "danger" : "primary"} />
          <View style={{ height: space.sm }} />
          <Button label={p.cancelLabel} onPress={p.onCancel} testID={`${p.testID}-cancel`} variant="ghost" />
        </View>
      </View>
    </Modal>
  );
}

export const s = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg },
  content: { padding: space.lg, paddingBottom: space.xl * 2 },
  card: { backgroundColor: colors.surface, borderColor: colors.border, borderWidth: 1, borderRadius: radius.md, padding: space.lg, marginBottom: space.md },
  cardTitle: { color: colors.muted, fontSize: 12, fontWeight: "700", letterSpacing: 1, textTransform: "uppercase", marginBottom: space.sm },
  btn: { borderRadius: radius.sm, paddingVertical: space.md, paddingHorizontal: space.lg, alignItems: "center", justifyContent: "center", minHeight: 46 },
  btn_primary: { backgroundColor: colors.accent },
  btn_secondary: { backgroundColor: colors.surface2, borderWidth: 1, borderColor: colors.border },
  btn_danger: { backgroundColor: colors.dangerStrong },
  btn_ghost: { backgroundColor: "transparent" },
  btnText: { color: colors.text, fontWeight: "600", fontSize: 15 },
  btnBig: { minHeight: 88, borderRadius: radius.lg },
  btnBigText: { fontSize: 22, fontWeight: "800", letterSpacing: 1.5, color: "#fff" },
  label: { color: colors.muted, fontSize: 13, marginBottom: space.xs },
  input: { backgroundColor: colors.surface2, color: colors.text, borderColor: colors.border, borderWidth: 1, borderRadius: radius.sm, paddingHorizontal: space.md, paddingVertical: space.sm, fontSize: 16 },
  row: { flexDirection: "row", alignItems: "center", paddingVertical: space.sm },
  h1: { color: colors.text, fontSize: 26, fontWeight: "700", marginBottom: space.md },
  h2: { color: colors.text, fontSize: 19, fontWeight: "700", marginBottom: space.sm },
  text: { color: colors.text, fontSize: 15, lineHeight: 21 },
  muted: { color: colors.muted, fontSize: 13, lineHeight: 18 },
  mono: { color: colors.text, fontFamily: "monospace", fontSize: 12 },
  error: { color: colors.danger, fontSize: 14, marginVertical: space.sm },
  badge: { borderWidth: 1, borderRadius: 999, paddingHorizontal: 8, paddingVertical: 2, fontSize: 11, fontWeight: "700", overflow: "hidden" },
  item: { flexDirection: "row", alignItems: "center", paddingVertical: space.md, borderBottomColor: colors.border, borderBottomWidth: StyleSheet.hairlineWidth },
  itemTitle: { color: colors.text, fontSize: 15, fontWeight: "600" },
  backdrop: { flex: 1, backgroundColor: "rgba(0,0,0,0.7)", justifyContent: "center", padding: space.xl },
  dialog: { backgroundColor: colors.surface, borderRadius: radius.lg, borderWidth: 1, borderColor: colors.border, padding: space.xl },
  stat: { color: colors.text, fontSize: 24, fontWeight: "700" },
  grid: { flexDirection: "row", flexWrap: "wrap", gap: space.md },
});

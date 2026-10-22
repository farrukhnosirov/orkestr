import { StatusBar } from "expo-status-bar";
import { StyleSheet, Text, View } from "react-native";

/** Placeholder shell (Phase 5 step 0); screens land in WP-C. */
export default function App() {
  return (
    <View style={styles.container} testID="app-root">
      <Text style={styles.title}>Orkestr</Text>
      <Text>Mobile client — coming together.</Text>
      <StatusBar style="auto" />
    </View>
  );
}

const styles = StyleSheet.create({
  container: { flex: 1, alignItems: "center", justifyContent: "center" },
  title: { fontSize: 24, fontWeight: "600", marginBottom: 8 },
});

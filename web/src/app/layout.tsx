import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "GNSS sky plot",
  description:
    "Client-side GPS sky plot from RINEX broadcast ephemeris, computed in WebAssembly.",
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}

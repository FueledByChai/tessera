import type { Metadata } from 'next';
import './globals.css';

export const metadata: Metadata = {
  title: 'Tessera Research Console',
  description: 'Local strategy research, immutable backtests, and production watchlists.',
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return <html lang="en"><body>{children}</body></html>;
}

import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { StatusCard } from './StatusCard';

describe('StatusCard', () => {
  it('renders title and value', () => {
    render(<StatusCard title="Uptime" value="1h" />);
    expect(screen.getByText('Uptime')).toBeDefined();
    expect(screen.getByText('1h')).toBeDefined();
  });

  it('renders hint when provided', () => {
    render(<StatusCard title="Cache" value="90%" hint="good" />);
    expect(screen.getByText('good')).toBeDefined();
  });

  it('applies healthy state classes', () => {
    render(<StatusCard title="T" value="ok" state="healthy" />);
    const card = screen.getByText('T').closest('div');
    expect(card?.className).toContain('border-emerald-500');
  });
});

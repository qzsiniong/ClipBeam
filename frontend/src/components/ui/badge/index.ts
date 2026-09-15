import { cva, type VariantProps } from 'class-variance-authority'

export { default as Badge } from './Badge.vue'

export const badgeVariants = cva(
  'inline-flex items-center rounded-md border px-2.5 py-0.5 text-xs font-semibold transition-colors focus:outline-none focus:ring-2 focus:ring-[var(--ring)] focus:ring-offset-2',
  {
    variants: {
      variant: {
        default: 'border-transparent bg-[var(--primary)] text-[var(--primary-foreground)] hover:bg-[var(--primary)]/80',
        secondary: 'border-transparent bg-[var(--secondary)] text-[var(--secondary-foreground)] hover:bg-[var(--secondary)]/80',
        destructive: 'border-transparent bg-[var(--destructive)] text-[var(--destructive-foreground)] hover:bg-[var(--destructive)]/80',
        outline: 'text-[var(--foreground)] border-[var(--border)]',
        success: 'border-transparent bg-[var(--ok)] text-white hover:bg-[var(--ok)]/80',
        warning: 'border-transparent bg-[var(--warn)] text-white hover:bg-[var(--warn)]/80',
      },
    },
    defaultVariants: {
      variant: 'default',
    },
  },
)

export type BadgeVariants = VariantProps<typeof badgeVariants>

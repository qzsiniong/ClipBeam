import { cva, type VariantProps } from 'class-variance-authority'

export { default as Button } from './Button.vue'

export const buttonVariants = cva(
  'inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--ring)] disabled:pointer-events-none disabled:opacity-50 cursor-pointer',
  {
    variants: {
      variant: {
        default: 'bg-[var(--primary)] text-[var(--primary-foreground)] hover:bg-[var(--primary)]/90',
        destructive: 'bg-[var(--destructive)] text-[var(--destructive-foreground)] hover:bg-[var(--destructive)]/90',
        outline: 'border border-[var(--border)] bg-transparent hover:bg-[var(--accent)] hover:text-[var(--accent-foreground)]',
        secondary: 'bg-[var(--secondary)] text-[var(--secondary-foreground)] hover:bg-[var(--secondary)]/80',
        ghost: 'hover:bg-[var(--accent)] hover:text-[var(--accent-foreground)]',
        link: 'text-[var(--primary)] underline-offset-4 hover:underline',
      },
      size: {
        default: 'h-9 px-4 py-2',
        sm: 'h-8 rounded-md px-3 text-xs',
        lg: 'h-10 rounded-md px-8',
        icon: 'h-9 w-9',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  },
)

export type ButtonVariants = VariantProps<typeof buttonVariants>

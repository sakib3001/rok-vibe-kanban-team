import { cn } from '../lib/cn';

interface ZohoLogoProps {
  className?: string;
}

/**
 * Zoho's "Z" logo mark in brand red (#E42527).
 */
export function ZohoLogo({ className }: ZohoLogoProps) {
  return (
    <svg
      className={cn('size-5', className)}
      viewBox="0 0 48 48"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
    >
      <path
        fill="#E42527"
        d="M4 8h30.4L4 36.8V40h40v-8H14.4L44 3.6V0H4v8z"
      />
    </svg>
  );
}

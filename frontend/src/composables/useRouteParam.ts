import { ref, watch, type Ref } from 'vue'
import { useRoute } from 'vue-router'

/**
 * The value of a route parameter (default `id`) that follows the address while this page stays mounted.
 *
 * Moving between two ids of the same route (course 1 -> course 2, a notification link, back/forward) re-uses the
 * component, so a page that read `route.params.id` once would keep showing the old record. The page watches the returned
 * ref instead, resets what it shows and loads the new record.
 *
 * The value is FROZEN once the route leaves this page (`route.name` changes): a page that is on its way out must not see
 * the next page's parameters and start loading the wrong thing.
 */
export function useRouteParam(name = 'id'): Ref<string> {
  const route = useRoute()
  const mine = route.name
  const read = (v: unknown): string => (typeof v === 'string' ? v : Array.isArray(v) ? String(v[0] ?? '') : '')
  const value = ref(read(route.params[name]))
  watch(
    () => [route.name, route.params[name]] as const,
    ([routeName, v]) => {
      if (routeName !== mine) return
      const next = read(v)
      if (next && next !== value.value) value.value = next
    },
  )
  return value
}

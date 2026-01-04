import { useQuery } from '@tanstack/react-query';
import { tasksApi } from '@/lib/api';
import type { Task } from 'shared/types';

export const taskChildrenKeys = {
  all: ['taskChildren'] as const,
  byTask: (taskId: string | undefined) => ['taskChildren', taskId] as const,
};

type Options = {
  enabled?: boolean;
  refetchInterval?: number | false;
  staleTime?: number;
  retry?: number | false;
};

export function useTaskChildren(taskId?: string, opts?: Options) {
  const enabled = (opts?.enabled ?? true) && !!taskId;

  return useQuery<Task[]>({
    queryKey: taskChildrenKeys.byTask(taskId),
    queryFn: async () => {
      const data = await tasksApi.getChildren(taskId!);
      return data;
    },
    enabled,
    refetchInterval: opts?.refetchInterval ?? false,
    staleTime: opts?.staleTime ?? 10_000,
    retry: opts?.retry ?? 2,
  });
}


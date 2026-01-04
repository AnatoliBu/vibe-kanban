import { useMemo } from 'react';
import NiceModal, { useModal } from '@ebay/nice-modal-react';
import { defineModal } from '@/lib/modals';
import { useTranslation } from 'react-i18next';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { PlusIcon } from 'lucide-react';
import { openTaskForm } from '@/lib/openTaskForm';
import { useTaskRelationships } from '@/hooks/useTaskRelationships';
import { useTaskChildren } from '@/hooks/useTaskChildren';
import { DataTable, type ColumnDef } from '@/components/ui/table/data-table';
import type { Task } from 'shared/types';
import type { Workspace } from 'shared/types';

export interface ViewRelatedTasksDialogProps {
  taskId: string;
  projectId: string;
  attemptId?: string;
  attempt?: Workspace | null;
  onNavigateToTask?: (taskId: string) => void;
}

const ViewRelatedTasksDialogImpl =
  NiceModal.create<ViewRelatedTasksDialogProps>(
    ({ taskId, attemptId, projectId, attempt, onNavigateToTask }) => {
      const modal = useModal();
      const { t } = useTranslation('tasks');
      const {
        data: relationships,
        isLoading,
        isError,
        refetch,
      } = useTaskRelationships(attemptId);

      const {
        data: taskChildren = [],
        isLoading: isLoadingChildren,
        isError: isErrorChildren,
        refetch: refetchChildren,
      } = useTaskChildren(taskId);

      // Combine parent and children into a single list of related tasks
      const relatedTasks = useMemo((): Task[] => {
        const byId = new Map<string, Task>();
        if (relationships?.parent_task) {
          byId.set(relationships.parent_task.id, relationships.parent_task);
        }
        if (relationships?.children) {
          for (const child of relationships.children) {
            byId.set(child.id, child);
          }
        }
        for (const child of taskChildren) {
          byId.set(child.id, child);
        }
        return Array.from(byId.values());
      }, [relationships?.children, relationships?.parent_task, taskChildren]);

      const taskColumns: ColumnDef<Task>[] = [
        {
          id: 'title',
          header: t('viewRelatedTasksDialog.columns.title'),
          accessor: (task) => (
            <div className="truncate" title={task.title}>
              {task.title || '—'}
            </div>
          ),
          className: 'pr-4',
          headerClassName: 'font-medium py-2 pr-4 w-1/2 bg-card',
        },
        {
          id: 'description',
          header: t('viewRelatedTasksDialog.columns.description'),
          accessor: (task) => (
            <div
              className="line-clamp-1 text-muted-foreground"
              title={task.description || ''}
            >
              {task.description?.trim() ? task.description : '—'}
            </div>
          ),
          className: 'pr-4',
          headerClassName: 'font-medium py-2 pr-4 bg-card',
        },
      ];

      const handleOpenChange = (open: boolean) => {
        if (!open) {
          modal.hide();
        }
      };

      const handleClickTask = (taskId: string) => {
        onNavigateToTask?.(taskId);
        modal.hide();
      };

      const handleCreateSubtask = async () => {
        if (!projectId || !attempt) return;

        // Close immediately - user intent is to create a subtask
        modal.hide();

        try {
          // Yield one microtask for smooth modal transition
          await Promise.resolve();

          await openTaskForm({
            mode: 'subtask',
            projectId,
            parentTaskAttemptId: attempt.id,
            initialBaseBranch: attempt.branch,
          });
        } catch {
          // User cancelled or error occurred
        }
      };

      return (
        <Dialog
          open={modal.visible}
          onOpenChange={handleOpenChange}
          className="max-w-3xl w-[92vw] p-0 overflow-x-hidden"
        >
          <DialogContent
            className="p-0 min-w-0"
            onKeyDownCapture={(e) => {
              if (e.key === 'Escape') {
                e.stopPropagation();
                modal.hide();
              }
            }}
          >
            <DialogHeader className="px-4 py-3 border-b">
              <DialogTitle>{t('viewRelatedTasksDialog.title')}</DialogTitle>
            </DialogHeader>

              <div className="p-4 max-h-[70vh] overflow-auto">
              {(isError || isErrorChildren) && (
                <div className="py-8 text-center space-y-3">
                  <div className="text-sm text-destructive">
                    {t('viewRelatedTasksDialog.error')}
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      void refetch();
                      void refetchChildren();
                    }}
                  >
                    {t('common:buttons.retry')}
                  </Button>
                </div>
              )}

              {!(isError || isErrorChildren) && (
                <DataTable
                  data={relatedTasks}
                  columns={taskColumns}
                  keyExtractor={(task) => task.id}
                  onRowClick={(task) => handleClickTask(task.id)}
                  isLoading={isLoading || isLoadingChildren}
                  emptyState={t('viewRelatedTasksDialog.empty')}
                  headerContent={
                    <div className="w-full flex text-left">
                      <span className="flex-1">
                        {t('viewRelatedTasksDialog.tasksCount', {
                          count: relatedTasks.length,
                        })}
                      </span>
                      <span>
                        <Button
                          variant="icon"
                          onClick={handleCreateSubtask}
                          disabled={!projectId || !attempt}
                        >
                          <PlusIcon size={16} />
                        </Button>
                      </span>
                    </div>
                  }
                />
              )}
            </div>
          </DialogContent>
        </Dialog>
      );
    }
  );

export const ViewRelatedTasksDialog = defineModal<
  ViewRelatedTasksDialogProps,
  void
>(ViewRelatedTasksDialogImpl);
